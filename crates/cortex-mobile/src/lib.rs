//! The phone apps' core.
//!
//! The app works in two ways, and the screens are the same for both. On
//! its own, the agent runs on the phone ([`local`]): the loop, the file
//! tools, the memory and the learning networks are here and only the
//! model is elsewhere. Connected to a desktop, the app is a second
//! window onto the desktop app and the agent runs there. This crate
//! holds the phone's agent, keeps the connection, holds everything the
//! screens show, and is the same for iOS and Android. The screens
//! themselves are written for each platform and only draw what they are
//! handed here.
//!
//! Nothing here waits: every call returns at once, and the screens are
//! told through [`Listener`] when there is something new to fetch.

mod driver;
mod local;
mod model;
mod view;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use cortex_core::Image;
use cortex_remote::wire::{ChatUp, Up};
use tokio::runtime::Runtime;
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

pub use model::Change;
pub use view::*;

use driver::Intent;
use local::{Ask, Local};
use model::Model;

uniffi::setup_scaffolding!();

/// Told when something the screens show has changed. Called from a
/// background thread.
#[uniffi::export(with_foreign)]
pub trait Listener: Send + Sync {
    fn changed(&self, changes: Vec<Change>);

    /// Show this web page to the person: the ChatGPT sign-in. The app
    /// has to stay running while it is shown, so it is opened inside the
    /// app and not handed to another one.
    fn open_address(&self, address: String);
}

/// What the driver and the screens share.
pub(crate) struct Shared {
    pub model: Mutex<Model>,
    pub listener: Arc<dyn Listener>,

    /// Where to send things for the desktop, while connected.
    pub link: Mutex<Option<UnboundedSender<Up>>>,

    /// The desktops this phone has been paired with.
    pub hosts: Mutex<Vec<HostView>>,

    /// The desktop that is chosen, connected or not.
    pub host: Mutex<Option<String>>,

    /// The agent on this phone, once it has been asked for.
    pub local: OnceLock<Arc<Local>>,

    /// How the sign-in to ChatGPT is going.
    pub sign_in: Mutex<SignIn>,
}

#[derive(Default)]
pub(crate) struct SignIn {
    busy: bool,
    problem: Option<String>,
}

impl Shared {
    pub fn model(&self) -> MutexGuard<'_, Model> {
        lock(&self.model)
    }

    /// The agent on this phone, started on first use. Called on the
    /// core's background threads.
    pub fn local(&self, folder: &Path) -> Arc<Local> {
        self.local.get_or_init(|| Local::start(folder)).clone()
    }

    pub fn tell(&self, changes: Vec<Change>) {
        if !changes.is_empty() {
            self.listener.changed(changes);
        }
    }
}

#[derive(uniffi::Object)]
pub struct Cortex {
    shared: Arc<Shared>,
    intents: UnboundedSender<Intent>,

    /// Keeps the agent's and the connection's tasks alive for as long
    /// as the app is.
    runtime: Runtime,
}

#[uniffi::export]
impl Cortex {
    /// `folder` is where the phone's key and the desktops it knows are
    /// kept: the app's own private folder. `device` is what the desktop
    /// lists this phone as. Connects to the desktop used last, if any.
    #[uniffi::constructor]
    pub fn new(folder: String, device: String, listener: Arc<dyn Listener>) -> Arc<Self> {
        make_home(&folder);

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("the app cannot run without its background threads");

        let shared = Arc::new(Shared {
            model: Mutex::default(),
            listener,
            link: Mutex::new(None),
            hosts: Mutex::default(),
            host: Mutex::new(None),
            local: OnceLock::new(),
            sign_in: Mutex::default(),
        });

        let (intents, asked) = unbounded_channel();

        runtime.spawn(driver::drive(
            shared.clone(),
            PathBuf::from(folder),
            device,
            asked,
        ));

        Arc::new(Self {
            shared,
            intents,
            runtime,
        })
    }

    /// The connection and the lists.
    pub fn state(&self) -> AppState {
        let model = self.shared.model();
        let host = lock(&self.shared.host).clone();

        AppState {
            connection: model.connection.clone().unwrap_or(Connection::Idle),
            hosts: lock(&self.shared.hosts).clone(),
            on_phone: host.as_deref() == Some(local::HOST),
            host,
            chats: model.chat_views(),
            projects: view::projects(&model.state.projects),
        }
    }

    /// The conversation of a chat.
    pub fn items(&self, chat: String) -> Vec<ItemView> {
        let model = self.shared.model();

        model
            .chats
            .get(&chat)
            .map(|chat| view::items(&chat.session))
            .unwrap_or_default()
    }

    /// What changed in the conversation of a chat since the screens were
    /// handed `revision`, which is 0 the first time. For a conversation
    /// that is followed as it grows: [`Cortex::items`] hands over all of
    /// it every time.
    pub fn items_since(&self, chat: String, revision: u64) -> ItemsDelta {
        let model = self.shared.model();

        model
            .chats
            .get(&chat)
            .map(|chat| chat.items_since(revision))
            .unwrap_or_default()
    }

    /// What a tool was handed and what it answered: number `index` of
    /// the tools of the item whose `id` is `item`. Asked for when the
    /// row is unfolded.
    pub fn tool_detail(&self, chat: String, item: u64, index: u32) -> Option<ToolDetailView> {
        let model = self.shared.model();

        view::tool_detail(
            &model.chats.get(&chat)?.session,
            usize::try_from(item).ok()?,
            index as usize,
        )
    }

    /// What was added to the response being written since the screens
    /// were last handed it. The three numbers are the ones the last
    /// [`WritingView`] came with, and 0 the first time.
    pub fn writing_since(
        &self,
        chat: String,
        response: u64,
        text_end: u64,
        thinking_end: u64,
    ) -> WritingView {
        let model = self.shared.model();

        model
            .chats
            .get(&chat)
            .map(|chat| chat.writing_since(response, text_end, thinking_end))
            .unwrap_or_default()
    }

    /// As [`Cortex::live`], without the text and the reasoning being
    /// written: [`Cortex::writing_since`] has those.
    pub fn live_status(&self, chat: String) -> LiveView {
        let model = self.shared.model();

        model
            .chats
            .get(&chat)
            .map(|chat| chat.live_without_text())
            .unwrap_or_default()
    }

    /// What changes while a run is in progress.
    pub fn live(&self, chat: String) -> LiveView {
        let model = self.shared.model();

        model
            .chats
            .get(&chat)
            .map(|chat| chat.live())
            .unwrap_or_default()
    }

    pub fn settings(&self, chat: String) -> SettingsView {
        let model = self.shared.model();

        model
            .settings(&chat)
            .map(view::settings)
            .unwrap_or_default()
    }

    pub fn memory(&self, chat: String) -> MemoryView {
        let model = self.shared.model();

        model
            .chats
            .get(&chat)
            .map(|chat| view::memory(&chat.memory))
            .unwrap_or_default()
    }

    /// One of the pictures sent with an instruction.
    pub fn picture(&self, chat: String, item: u64, index: u32) -> Option<Vec<u8>> {
        use cortex_remote::transcript::Item;

        let model = self.shared.model();

        let item = model.chats.get(&chat)?.session.items.get(item as usize)?;

        let Item::User { images, .. } = item else {
            return None;
        };

        STANDARD.decode(&images.get(index as usize)?.data).ok()
    }

    /// Pairs with the desktop whose code this is, read from its screen,
    /// and connects to it.
    pub fn pair(&self, code: String) {
        self.intend(Intent::Pair(code));
    }

    /// Connects to a desktop this phone has been paired with.
    pub fn connect(&self, host: String) {
        self.intend(Intent::Connect(host));
    }

    /// Works with the agent on this phone instead of a desktop's.
    pub fn use_phone(&self) {
        self.intend(Intent::Local);
    }

    /// Who the model on this phone is reached as: the provider, whether
    /// it is signed in to or has a key, and how a sign-in is going.
    pub fn account(&self) -> AccountView {
        let settings = match self.shared.local.get() {
            Some(engine) => engine.settings(),
            None => local::Settings::default(),
        };

        let sign_in = lock(&self.shared.sign_in);

        AccountView {
            provider: settings.provider.clone(),
            model: settings.model(),
            signed_in: cortex_providers::codex::sign_in_status().is_some(),
            signing_in: sign_in.busy,
            sign_in_problem: sign_in.problem.clone(),
            has_key: settings.key().is_some(),
            address: settings.address.clone(),

            providers: local::PROVIDERS
                .iter()
                .map(|(name, label)| ProviderView {
                    name: name.to_string(),
                    label: label.to_string(),
                })
                .collect(),
        }
    }

    /// Keeps the API key of a provider, on this phone only.
    pub fn set_key(&self, provider: String, key: String) {
        self.ask_phone(Ask::SetKey { provider, key });
    }

    /// The address of the hosted provider's API.
    pub fn set_address(&self, address: String) {
        self.ask_phone(Ask::SetAddress(address));
    }

    /// Makes a project on this phone and works in it.
    pub fn new_project(&self, name: String) {
        self.ask_phone(Ask::NewProject(name));
    }

    /// Signs in to ChatGPT, to use a subscription's models on this
    /// phone. The listener is handed the page to show; the password is
    /// only ever typed into that page.
    pub fn sign_in(&self) {
        {
            let mut sign_in = lock(&self.shared.sign_in);

            if sign_in.busy {
                return;
            }

            *sign_in = SignIn {
                busy: true,
                problem: None,
            };
        }

        self.shared.tell(vec![Change::State]);

        let shared = self.shared.clone();

        self.runtime.spawn(async move {
            let listener = shared.listener.clone();

            let signed_in = cortex_providers::codex::login(|address| {
                listener.open_address(address.to_string())
            })
            .await;

            *lock(&shared.sign_in) = SignIn {
                busy: false,
                problem: signed_in.err().map(|error| error.to_string()),
            };

            if let Some(engine) = shared.local.get() {
                engine.ask(Ask::Reload);
            }

            shared.tell(vec![Change::State]);
        });
    }

    /// Forgets the ChatGPT sign-in kept on this phone.
    pub fn sign_out(&self) {
        if let Some(file) = local::sign_in_file() {
            let _ = std::fs::remove_file(file);
        }

        self.ask_phone(Ask::Reload);
        self.shared.tell(vec![Change::State]);
    }

    pub fn disconnect(&self) {
        self.intend(Intent::Disconnect);
    }

    /// Forgets a desktop. It has to be paired with again.
    pub fn forget(&self, host: String) {
        self.intend(Intent::Forget(host));
    }

    /// The app is on screen again: a connection that was lost while it
    /// was not is tried again at once.
    pub fn foreground(&self) {
        self.intend(Intent::Foreground);
    }

    pub fn submit(&self, chat: String, text: String, pictures: Vec<Picture>) {
        let images = pictures
            .into_iter()
            .map(|picture| Image {
                media_type: picture.media_type,
                data: STANDARD.encode(picture.data),
            })
            .collect();

        self.ask(chat, ChatUp::Submit { text, images });
    }

    /// Tells the run in progress something. It waits, shown as queued,
    /// until the model has read it.
    pub fn steer(&self, chat: String, text: String) {
        if let Some(held) = self.shared.model().chats.get_mut(&chat) {
            held.queued.push(text.clone());
        }

        self.shared.tell(vec![Change::Live { chat: chat.clone() }]);

        self.ask(chat, ChatUp::Steer { text });
    }

    pub fn stop(&self, chat: String) {
        self.ask(chat, ChatUp::Stop);
    }

    /// Carries on the run that stopped on a provider failure.
    pub fn carry_on(&self, chat: String) {
        self.ask(chat, ChatUp::Continue);
    }

    /// Answers a tool's question.
    pub fn answer(&self, chat: String, id: u64, answer: Answer) {
        // Taken off the screen at once; the desktop confirms it.
        if let Some(held) = self.shared.model().chats.get_mut(&chat) {
            held.approvals.retain(|approval| approval.id != id);
        }

        self.shared
            .tell(vec![Change::Live { chat: chat.clone() }, Change::State]);

        self.ask(
            chat,
            ChatUp::Approve {
                id,
                decision: answer.into(),
            },
        );
    }

    /// Starts a new session, in `folder` or where the chat already is.
    pub fn new_session(&self, chat: String, folder: Option<String>) {
        self.ask(chat, ChatUp::NewSession { folder });
    }

    /// Takes up an earlier session.
    pub fn open_session(&self, chat: String, path: String) {
        self.ask(chat, ChatUp::OpenSession { path });
    }

    pub fn set_capability(&self, chat: String, name: String, on: bool) {
        self.change(&chat, |settings| {
            if let Some(capability) = settings
                .capabilities
                .iter_mut()
                .find(|capability| capability.name == name)
            {
                capability.on = on;
            }
        });

        self.ask(chat, ChatUp::SetCapability { name, on });
    }

    pub fn set_model(&self, chat: String, provider: String, model: String) {
        self.change(&chat, |settings| {
            settings.provider = provider.clone();
            settings.model = model.clone();
        });

        self.ask(chat, ChatUp::SetModel { provider, model });
    }

    pub fn set_effort(&self, chat: String, effort: String) {
        self.change(&chat, |settings| settings.effort = effort.clone());

        self.ask(chat, ChatUp::SetEffort { effort });
    }

    pub fn set_approve_everything(&self, chat: String, on: bool) {
        self.change(&chat, |settings| settings.approve_everything = on);

        self.ask(chat, ChatUp::SetApproveEverything { on });
    }
}

impl Cortex {
    fn intend(&self, intent: Intent) {
        // The driver lives as long as this does.
        let _ = self.intents.send(intent);
    }

    /// Asks the agent on this phone, when it has been started.
    fn ask_phone(&self, ask: Ask) {
        if let Some(engine) = self.shared.local.get() {
            engine.ask(ask);
        }
    }

    fn ask(&self, chat: String, command: ChatUp) {
        if let Some(link) = &*lock(&self.shared.link) {
            let _ = link.send(Up::Chat { chat, command });
        }
    }

    /// Shows a changed setting at once. The desktop's own account of the
    /// settings follows and replaces it.
    fn change(&self, chat: &str, change: impl FnOnce(&mut cortex_remote::wire::SettingsView)) {
        if let Some(settings) = self.shared.model().settings_mut(chat) {
            change(settings);
        }

        self.shared.tell(vec![Change::State]);
    }
}

/// Lets the growing network on this phone go over its recent moments
/// for up to `seconds`, and saves it. For the system's background work:
/// it needs no screen and no connection, and returns when it is done.
/// `folder` is the one the app gives [`Cortex::new`].
#[uniffi::export]
pub fn rest(folder: String, seconds: u32) -> RestView {
    make_home(&folder);

    match local::rest(Duration::from_secs(seconds.into())) {
        Some(rested) => RestView {
            replays: rested.replays,
            neurons: rested.neurons,
            synapses: rested.synapses,
        },

        None => RestView::default(),
    }
}

/// The engine keeps what it learns across projects, and the ChatGPT
/// sign-in, in `.cortex` under the home folder. A phone has none it can
/// write to, so the app's own folder stands in for it.
fn make_home(folder: &str) {
    if cfg!(any(target_os = "android", target_os = "ios"))
        || std::env::var_os("CORTEX_PHONE_HOME").is_some()
    {
        static MADE: OnceLock<()> = OnceLock::new();

        MADE.get_or_init(|| {
            // Before any other thread of the core exists to read it.
            unsafe { std::env::set_var("HOME", folder) };
        });
    }
}

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}
