//! Keeping the connection to the desktop: making it, following what
//! comes over it, and making it again when it is lost.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use cortex_remote::Client;
use cortex_remote::link::Session;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::time::{Instant, sleep, sleep_until};

use crate::local::{self, Feed};
use crate::model::Change;
use crate::view::{Connection, HostView};
use crate::{Shared, lock};

/// The file the phone's key and its desktops are kept in.
const KEYS: &str = "remote.json";

/// The file that names the desktop used last.
const LAST: &str = "last-host";

/// Waits between tries to connect again: short at first, then longer.
const WAITS: [u64; 6] = [1, 2, 3, 5, 8, 12];

/// The screens are told of a growing conversation at most this often: a
/// model writes, and its tools report, faster than a screen is drawn.
const BREATH: Duration = Duration::from_millis(32);

/// What the person asked for.
pub enum Intent {
    Pair(String),
    Connect(String),
    Disconnect,
    Forget(String),
    Foreground,

    /// Work with the agent on this phone.
    Local,
}

/// How a connection is to be made.
enum Dial {
    /// With a pairing code, once.
    Code(String),

    /// To a desktop that is already paired, for as long as it takes.
    Known(String),

    /// To the agent on this phone, which is always there.
    Local,
}

/// Where what the screens show comes from.
enum Source {
    Desktop(Session),
    Phone(Feed),
}

impl Source {
    async fn next(&mut self) -> Option<cortex_remote::wire::Down> {
        match self {
            Self::Desktop(session) => session.next().await,
            Self::Phone(feed) => feed.next().await,
        }
    }

    fn waiting(&mut self) -> Option<cortex_remote::wire::Down> {
        match self {
            Self::Desktop(session) => session.waiting(),
            Self::Phone(feed) => feed.waiting(),
        }
    }
}

pub async fn drive(
    shared: Arc<Shared>,
    folder: PathBuf,
    device: String,
    mut intents: UnboundedReceiver<Intent>,
) {
    let client = loop {
        match Client::open(&folder.join(KEYS)).await {
            Ok(client) => break client,

            Err(error) => {
                connection(
                    &shared,
                    Connection::Failed {
                        reason: format!("{error:#}"),
                    },
                );

                // Nothing works without it. An intent is a reason to try again.
                if intents.recv().await.is_none() {
                    return;
                }
            }
        }
    };

    list_hosts(&shared, &client);

    // Carry on with the desktop used last.
    let last = std::fs::read_to_string(folder.join(LAST)).ok();

    let mut dial = match last {
        Some(id) if id == local::HOST => Some(Dial::Local),

        last => last
            .filter(|id| client.hosts().iter().any(|host| host.id == *id))
            .map(Dial::Known),
    };

    let mut tries = 0;

    loop {
        let Some(target) = dial.take() else {
            *lock(&shared.host) = None;

            if !matches!(shared.model().connection, Some(Connection::Failed { .. })) {
                connection(&shared, Connection::Idle);
            }

            match intents.recv().await {
                Some(intent) => dial = decide(intent, &shared, &client, None),
                None => return,
            }

            continue;
        };

        let target = match target {
            Dial::Local => {
                let engine = shared.local(&folder);

                // The engine says everything again from the start.
                shared.model().clear();

                let (feed, sender) = engine.attach();

                *lock(&shared.host) = Some(local::HOST.to_string());

                let _ = std::fs::write(folder.join(LAST), local::HOST);

                *lock(&shared.link) = Some(sender);

                connection(
                    &shared,
                    Connection::Connected {
                        host: local::NAME.to_string(),
                    },
                );

                let ended = follow(&shared, Source::Phone(feed), &mut intents).await;

                *lock(&shared.link) = None;

                dial = match ended {
                    // Fell too far behind: start over from what is kept.
                    Ended::Lost => Some(Dial::Local),
                    Ended::Asked(intent) => decide(intent, &shared, &client, Some(Dial::Local)),
                    Ended::Closed => return,
                };

                tries = 0;

                continue;
            }

            desktop => desktop,
        };

        let name = match &target {
            Dial::Known(id) => host_name(&client, id),
            Dial::Code(_) | Dial::Local => "the desktop".to_string(),
        };

        if let Dial::Known(id) = &target {
            *lock(&shared.host) = Some(id.clone());
        }

        if tries == 0 {
            connection(&shared, Connection::Connecting { host: name.clone() });
        }

        let dialing = async {
            match &target {
                Dial::Code(code) => client.pair(code, &device).await,
                Dial::Known(id) => client.connect(id, &device).await,
                Dial::Local => unreachable!("the phone's own agent is not dialed"),
            }
        };

        // An intent that arrives meanwhile is not kept waiting.
        let dialed = tokio::select! {
            dialed = dialing => dialed,

            intent = intents.recv() => match intent {
                Some(intent) => {
                    dial = decide(intent, &shared, &client, Some(target));
                    tries = 0;

                    continue;
                }

                None => return,
            },
        };

        let session = match (dialed, target) {
            (Ok(session), Dial::Code(code)) => {
                list_hosts(&shared, &client);

                // The desktop the code was for is now the one in use.
                let id = cortex_remote::Ticket::decode(&code)
                    .map(|ticket| ticket.addr.id.to_string())
                    .unwrap_or_default();

                (session, id)
            }

            (Ok(session), Dial::Known(id)) => (session, id),

            (_, Dial::Local) => unreachable!("the phone's own agent is not dialed"),

            // A code works once: there is nothing to try again with.
            (Err(error), Dial::Code(_)) => {
                connection(
                    &shared,
                    Connection::Failed {
                        reason: format!("{error:#}"),
                    },
                );

                continue;
            }

            (Err(error), Dial::Known(id)) => {
                let reason = format!("{error:#}");

                // The desktop has forgotten this phone. Trying again
                // would be refused again.
                if reason.contains("not paired") {
                    connection(&shared, Connection::Failed { reason });

                    continue;
                }

                connection(&shared, Connection::Reconnecting { host: name, reason });

                let wait = WAITS[tries.min(WAITS.len() - 1)];

                tries += 1;

                dial = tokio::select! {
                    _ = sleep(Duration::from_secs(wait)) => Some(Dial::Known(id)),

                    intent = intents.recv() => match intent {
                        Some(intent) => {
                            tries = 0;

                            decide(intent, &shared, &client, Some(Dial::Known(id)))
                        }

                        None => return,
                    },
                };

                continue;
            }
        };

        let (session, id) = session;

        tries = 0;

        *lock(&shared.host) = Some(id.clone());

        let _ = std::fs::write(folder.join(LAST), &id);

        *lock(&shared.link) = Some(session.sender());

        connection(
            &shared,
            Connection::Connected {
                host: session.host.clone(),
            },
        );

        let ended = follow(&shared, Source::Desktop(session), &mut intents).await;

        *lock(&shared.link) = None;

        dial = match ended {
            // Lost: the same desktop is tried again.
            Ended::Lost => {
                connection(
                    &shared,
                    Connection::Reconnecting {
                        host: host_name(&client, &id),
                        reason: "The connection was lost.".to_string(),
                    },
                );

                tries = 1;

                Some(Dial::Known(id))
            }

            Ended::Asked(intent) => decide(intent, &shared, &client, Some(Dial::Known(id))),
            Ended::Closed => return,
        };
    }
}

enum Ended {
    Lost,
    Asked(Intent),

    /// The app is gone.
    Closed,
}

/// Takes in what the desktop says until the connection is lost or the
/// person asks for something that ends it.
async fn follow(
    shared: &Shared,
    mut session: Source,
    intents: &mut UnboundedReceiver<Intent>,
) -> Ended {
    let mut pace = Pace::new(Instant::now());

    loop {
        tokio::select! {
            message = session.next() => {
                let Some(message) = message else {
                    shared.tell(pace.take_all());

                    return Ended::Lost;
                };

                let mut model = shared.model();

                pace.add(model.apply(message));

                // Whatever else has already arrived is taken in under
                // the same lock, and told about once.
                while let Some(message) = session.waiting() {
                    pace.add(model.apply(message));
                }

                drop(model);

                if let Some(changes) = pace.take(Instant::now()) {
                    shared.tell(changes);
                }
            }

            _ = sleep_until(pace.due()), if pace.is_waiting() => {
                if let Some(changes) = pace.take(Instant::now()) {
                    shared.tell(changes);
                }
            }

            intent = intents.recv() => match intent {
                // Still connected: there is nothing to do.
                Some(Intent::Foreground) => {}

                Some(intent) => return Ended::Asked(intent),
                None => return Ended::Closed,
            },
        }
    }
}

/// Holds back what the screens are to be told, so that a burst of
/// messages wakes them once. A conversation that grows and the text
/// being written are told of at most once a [`BREATH`]; the connection,
/// the list of chats and a tool's question are told of at once, and
/// take whatever was waiting with them.
struct Pace {
    /// Changes not yet told.
    pending: HashSet<Change>,

    /// When the screens were last told.
    told: Instant,
}

impl Pace {
    /// The first thing to tell does not wait.
    fn new(now: Instant) -> Self {
        Self {
            pending: HashSet::new(),
            told: now.checked_sub(BREATH).unwrap_or(now),
        }
    }

    fn add(&mut self, changes: Vec<Change>) {
        self.pending.extend(changes);
    }

    fn is_waiting(&self) -> bool {
        !self.pending.is_empty()
    }

    /// When what is waiting has waited long enough.
    fn due(&self) -> Instant {
        self.told + BREATH
    }

    /// What to tell the screens at `now`, if it is time to.
    fn take(&mut self, now: Instant) -> Option<Vec<Change>> {
        let urgent = self.pending.contains(&Change::State);

        if self.pending.is_empty() || !(urgent || now >= self.due()) {
            return None;
        }

        self.told = now;

        Some(self.take_all())
    }

    fn take_all(&mut self) -> Vec<Change> {
        self.pending.drain().collect()
    }
}

/// What to connect to after `intent`. `current` is what was being
/// connected to, or was connected.
fn decide(intent: Intent, shared: &Shared, client: &Client, current: Option<Dial>) -> Option<Dial> {
    match intent {
        Intent::Pair(code) => {
            forget_what_was_shown(shared);

            Some(Dial::Code(code))
        }

        Intent::Connect(id) => {
            forget_what_was_shown(shared);

            Some(Dial::Known(id))
        }

        Intent::Local => {
            forget_what_was_shown(shared);

            Some(Dial::Local)
        }

        Intent::Disconnect => {
            forget_what_was_shown(shared);

            connection(shared, Connection::Idle);

            None
        }

        Intent::Forget(id) => {
            let _ = client.forget(&id);

            list_hosts(shared, client);

            match current {
                Some(Dial::Known(current)) if current == id => {
                    forget_what_was_shown(shared);

                    connection(shared, Connection::Idle);

                    None
                }

                current => current,
            }
        }

        // Try at once instead of sitting out the wait.
        Intent::Foreground => current,
    }
}

fn forget_what_was_shown(shared: &Shared) {
    shared.model().clear();
}

fn connection(shared: &Shared, connection: Connection) {
    shared.model().connection = Some(connection);

    shared.tell(vec![Change::State]);
}

fn list_hosts(shared: &Shared, client: &Client) {
    *lock(&shared.hosts) = client
        .hosts()
        .into_iter()
        .map(|host| HostView {
            id: host.id,
            name: host.name,
        })
        .collect();

    shared.tell(vec![Change::State]);
}

fn host_name(client: &Client, id: &str) -> String {
    client
        .hosts()
        .into_iter()
        .find(|host| host.id == id)
        .map(|host| host.name)
        .unwrap_or_else(|| "the desktop".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn live() -> Change {
        Change::Live {
            chat: "phone".to_string(),
        }
    }

    fn items() -> Change {
        Change::Items {
            chat: "phone".to_string(),
        }
    }

    #[test]
    fn a_burst_of_text_and_events_wakes_the_screens_once_a_breath() {
        let start = Instant::now();
        let mut pace = Pace::new(start);
        let mut told = Vec::new();

        // A piece of text and an event every millisecond, for a second.
        for millisecond in 0..1000 {
            let now = start + Duration::from_millis(millisecond);

            pace.add(vec![live(), items()]);

            if let Some(changes) = pace.take(now) {
                told.push((now, changes));
            }
        }

        // The first is told at once, and then one a breath.
        assert_eq!(told[0].0, start);
        assert_eq!(told.len(), 1000 / BREATH.as_millis() as usize + 1);

        for (_, changes) in &told {
            assert_eq!(changes.len(), 2, "each kind of change is told once");
        }

        for pair in told.windows(2) {
            assert!(pair[1].0 - pair[0].0 >= BREATH);
        }
    }

    #[test]
    fn what_was_held_back_is_told_when_its_time_comes() {
        let start = Instant::now();
        let mut pace = Pace::new(start);

        pace.add(vec![live()]);

        assert!(pace.take(start).is_some());

        pace.add(vec![live()]);

        assert_eq!(pace.take(start + Duration::from_millis(5)), None);
        assert!(pace.is_waiting());
        assert_eq!(pace.due(), start + BREATH);
        assert_eq!(pace.take(pace.due()), Some(vec![live()]));
        assert!(!pace.is_waiting());
    }

    #[test]
    fn a_question_or_the_list_of_chats_does_not_wait() {
        let start = Instant::now();
        let mut pace = Pace::new(start);

        pace.add(vec![items()]);
        pace.take(start);

        pace.add(vec![items(), Change::State]);

        let changes = pace.take(start + Duration::from_millis(1)).unwrap();

        assert_eq!(changes.len(), 2);
    }
}
