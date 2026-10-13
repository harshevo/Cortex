use std::fs;
use std::io::Read;
use std::path::Path;

use cortex_core::AutoApprove;

use super::*;

const PAGE: &str = r#"<!doctype html>
<html>
<head>
  <title>Orders</title>
  <style>
    .cover { position: fixed; inset: 0; background: white; display: none; }
    .cover.up { display: block; }
    .card { cursor: pointer; padding: 8px; }
  </style>
</head>
<body>
  <nav aria-label="Main"><a href="other.html">Other page</a></nav>
  <main>
    <h1>Orders</h1>
    <p>You have <b id="count">0</b> orders.</p>
    <form onsubmit="event.preventDefault(); document.getElementById('greeting').textContent = 'Hello ' + this.who.value + ' (' + this.size.value + ')';">
      <label>Name <input name="who" value="old"></label>
      <label>Size <select name="size"><option value="s">Small</option><option value="l">Large</option></select></label>
      <label><input type="checkbox" name="rush"> Rush</label>
      <input type="password" name="secret" value="hunter2">
      <button type="submit">Greet</button>
    </form>
    <p id="greeting"></p>
    <button id="add" onclick="document.getElementById('count').textContent = Number(document.getElementById('count').textContent) + 1">Add order</button>
    <button id="break" onclick="console.error('order failed', {code: 7}); console.log('fine'); nothing.here()">Break</button>
    <button id="later" onclick="setTimeout(() => { const p = document.createElement('p'); p.textContent = 'Shipped at last'; document.body.append(p); }, 400)">Ship</button>
    <button id="ask" onclick="document.getElementById('greeting').textContent = confirm('Really?') ? 'confirmed' : 'refused'">Ask</button>
    <button id="hide" onclick="document.querySelector('.cover').classList.add('up')">Cover</button>
    <button disabled>Frozen</button>
    <div class="card" onclick="this.dataset.hit = 1">A card you can click</div>
    <table><tr><th>Item</th><th>Price</th></tr><tr><td>Tea</td><td>4</td></tr></table>
    <p style="display: none">Never shown</p>
    <img src="missing.png" alt="A missing picture">
  </main>
  <div class="cover">Please wait</div>
  <script>var tucked = "not on the page";</script>
</body>
</html>"#;

const OTHER: &str = "<!doctype html><title>Other</title><h1>Elsewhere</h1>\
                     <a href=\"index.html\" target=\"_blank\">Home in a new tab</a>";

fn context(root: &Path) -> ToolContext {
    ToolContext {
        workspace: Workspace::new(root).unwrap(),
        approver: Arc::new(AutoApprove),
    }
}

/// A project holding the test pages, or nothing on a machine that has no
/// browser to open them with.
fn project() -> Option<tempfile::TempDir> {
    if chrome::find_program().is_err() {
        eprintln!("skipped: no browser is installed");

        return None;
    }

    let directory = tempfile::tempdir().unwrap();

    fs::write(directory.path().join("index.html"), PAGE).unwrap();
    fs::write(directory.path().join("other.html"), OTHER).unwrap();

    Some(directory)
}

/// The number the snapshot gives the control whose line contains `label`.
fn number_of(snapshot: &str, label: &str) -> u64 {
    let line = snapshot
        .lines()
        .find(|line| line.trim_start().starts_with('[') && line.contains(label))
        .unwrap_or_else(|| panic!("no line with {label:?} in:\n{snapshot}"));

    let inside = line
        .trim_start()
        .strip_prefix('[')
        .unwrap_or_else(|| panic!("no number on: {line}"));

    inside[..inside.find(']').unwrap()].parse().unwrap()
}

#[tokio::test]
async fn a_page_is_read_acted_on_and_checked() {
    let Some(project) = project() else { return };

    let context = context(project.path());
    let (browser, view) = tools(BrowserOptions {
        show_window: false,
        pictures: true,
    });

    let act = |input: Value| browser.run(input, &context);
    let look = |input: Value| view.run(input, &context);

    // Nothing to look at before a page is open.
    let early = look(json!({ "action": "snapshot" })).await.unwrap_err();

    assert!(early.to_string().contains("no page is open"), "{early}");

    let opened = act(json!({ "action": "open", "url": "index.html" }))
        .await
        .unwrap();

    assert!(opened.contains("Page: Orders"), "{opened}");
    assert!(opened.contains("navigation \"Main\""), "{opened}");
    assert!(opened.contains("heading 1 \"Orders\""), "{opened}");
    assert!(opened.contains("text \"You have 0 orders.\""), "{opened}");
    assert!(opened.contains("row Tea | 4"), "{opened}");
    assert!(opened.contains("image \"A missing picture\""), "{opened}");
    assert!(opened.contains("link \"Other page\" → /"), "{opened}");
    assert!(
        opened.contains("textbox \"Name\" value=\"old\""),
        "{opened}"
    );
    assert!(
        opened.contains("combobox \"Size\" value=\"Small\", options: Small | Large"),
        "{opened}"
    );
    assert!(opened.contains("checkbox \"Rush\" unchecked"), "{opened}");
    assert!(opened.contains("button \"Frozen\" disabled"), "{opened}");
    assert!(
        opened.contains("clickable \"A card you can click\""),
        "{opened}"
    );

    // Hidden text, scripts and what a password field holds stay out.
    assert!(!opened.contains("Never shown"), "{opened}");
    assert!(!opened.contains("tucked"), "{opened}");
    assert!(!opened.contains("hunter2"), "{opened}");

    // The picture that is not there was noticed as the page loaded.
    assert!(opened.contains("1 failed request"), "{opened}");

    let failures = look(json!({ "action": "network" })).await.unwrap();

    assert!(
        failures.contains("failed (net::ERR_FILE_NOT_FOUND) GET"),
        "{failures}"
    );
    assert!(failures.contains("missing.png"), "{failures}");

    let every = look(json!({ "action": "network", "filter": "index" }))
        .await
        .unwrap();

    assert!(every.contains("index.html"), "{every}");

    // A click changes the page, and the answer shows the page after it.
    let add = number_of(&opened, "Add order");
    let clicked = act(json!({ "action": "click", "ref": add })).await.unwrap();

    assert!(
        clicked.starts_with("Clicked button \"Add order\"."),
        "{clicked}"
    );
    assert!(clicked.contains("You have 1 orders."), "{clicked}");

    // The same element keeps its number from one snapshot to the next.
    assert_eq!(number_of(&clicked, "Add order"), add);

    // Typing replaces what the field held; a list is chosen from by text.
    let name = number_of(&clicked, "textbox \"Name\"");
    let typed = act(json!({ "action": "type", "ref": name, "text": "Ada" }))
        .await
        .unwrap();

    assert_eq!(typed, "Typed \"Ada\" into textbox \"Name\".");

    let chosen =
        act(json!({ "action": "select", "selector": "select[name=size]", "value": "large" }))
            .await
            .unwrap();

    assert!(chosen.contains("Chose \"Large\""), "{chosen}");

    let appended = act(json!({
        "action": "type", "ref": name, "text": " L", "append": true, "submit": true,
    }))
    .await
    .unwrap();

    assert!(appended.contains("text \"Hello Ada L (l)\""), "{appended}");

    let rush = number_of(&appended, "checkbox \"Rush\"");
    let ticked = act(json!({ "action": "click", "ref": rush }))
        .await
        .unwrap();

    assert!(ticked.contains("checkbox \"Rush\" checked"), "{ticked}");

    // Keys go to whatever has focus: Tab moves on from the checkbox.
    let tabbed = act(json!({ "action": "press", "key": "Tab" }))
        .await
        .unwrap();

    assert!(
        tabbed.contains("type=password, value=\"•••••••\", focused"),
        "{tabbed}"
    );

    // An error the page logs or throws is mentioned at once, and read in
    // full on request.
    let broke = act(json!({ "action": "click", "selector": "#break" }))
        .await
        .unwrap();

    assert!(broke.contains("2 new errors in the console"), "{broke}");

    let console = look(json!({ "action": "console" })).await.unwrap();

    assert!(console.contains("error: order failed"), "{console}");
    assert!(console.contains("log: fine"), "{console}");
    assert!(
        console.contains("error: uncaught ReferenceError: nothing is not defined"),
        "{console}"
    );
    assert!(console.contains("index.html:"), "{console}");

    let quiet = look(json!({ "action": "console" })).await.unwrap();

    assert!(quiet.contains("logged nothing"), "{quiet}");

    // Something that turns up later is waited for.
    act(json!({ "action": "click", "selector": "#later" }))
        .await
        .unwrap();

    let waited = look(json!({ "action": "wait", "text": "Shipped at last", "seconds": 5 }))
        .await
        .unwrap();

    assert!(waited.contains("is there after"), "{waited}");

    let never = look(json!({ "action": "wait", "selector": "#nowhere", "seconds": 1 }))
        .await
        .unwrap_err();

    assert!(
        never.to_string().contains("is not there after 1 seconds"),
        "{never}"
    );

    // A dialog would stop the page; it is accepted and noted.
    let asked = act(json!({ "action": "click", "selector": "#ask" }))
        .await
        .unwrap();

    assert!(asked.contains("text \"confirmed\""), "{asked}");
    assert!(
        look(json!({ "action": "console" }))
            .await
            .unwrap()
            .contains("confirm box saying \"Really?\"")
    );

    // A script's value comes back, written either way.
    let value = act(json!({ "action": "evaluate", "script": "({ orders: Number(document.getElementById('count').textContent) })" }))
        .await
        .unwrap();

    assert!(value.contains("\"orders\": 1"), "{value}");

    let returned = act(json!({ "action": "evaluate", "script": "const n = 2; return n * 21;" }))
        .await
        .unwrap();

    assert!(returned.contains("42"), "{returned}");

    let thrown = act(json!({ "action": "evaluate", "script": "missing.thing" }))
        .await
        .unwrap_err();

    assert!(
        thrown.to_string().contains("missing is not defined"),
        "{thrown}"
    );

    // A click that could not land is refused, as it would be for a person.
    let frozen = act(json!({ "action": "click", "ref": number_of(&asked, "Frozen") }))
        .await
        .unwrap_err();

    assert!(
        frozen.to_string().contains("button \"Frozen\" is disabled"),
        "{frozen}"
    );

    act(json!({ "action": "click", "selector": "#hide" }))
        .await
        .unwrap();

    let covered = act(json!({ "action": "click", "ref": add }))
        .await
        .unwrap_err();

    assert!(covered.to_string().contains("is covered by"), "{covered}");

    let nothing = act(json!({ "action": "click", "selector": "#nowhere" }))
        .await
        .unwrap_err();

    assert!(
        nothing.to_string().contains("nothing on the page matches"),
        "{nothing}"
    );

    // One part of the page can be read on its own.
    let part = look(json!({ "action": "snapshot", "selector": "table" }))
        .await
        .unwrap();

    assert!(part.contains("row Tea | 4"), "{part}");
    assert!(!part.contains("Add order"), "{part}");

    browser
        .run(json!({ "action": "close" }), &context)
        .await
        .unwrap();
}

#[tokio::test]
async fn pages_are_moved_between_and_photographed() {
    let Some(project) = project() else { return };

    let context = context(project.path());
    let (browser, view) = tools(BrowserOptions {
        show_window: false,
        pictures: true,
    });

    let act = |input: Value| browser.run(input, &context);

    let first = act(json!({ "action": "open", "url": "index.html" }))
        .await
        .unwrap();
    let add = number_of(&first, "Add order");

    // Following a link loads another document.
    let followed = act(json!({ "action": "click", "ref": number_of(&first, "Other page") }))
        .await
        .unwrap();

    assert!(followed.contains("Page: Other"), "{followed}");
    assert!(followed.contains("heading 1 \"Elsewhere\""), "{followed}");

    // Numbers are not used twice, so one from the page before points at
    // nothing rather than at something else.
    let link = number_of(&followed, "Home in a new tab");

    assert!(link > add);

    let stale = act(json!({ "action": "click", "ref": add }))
        .await
        .unwrap_err();

    assert!(stale.to_string().contains("take a new snapshot"), "{stale}");

    // A link meant for a new tab opens in the one page there is.
    let home = act(json!({ "action": "click", "ref": link }))
        .await
        .unwrap();

    assert!(home.contains("Page: Orders"), "{home}");

    let back = act(json!({ "action": "back" })).await.unwrap();

    assert!(back.contains("Page: Other"), "{back}");

    let again = act(json!({ "action": "reload" })).await.unwrap();

    assert!(again.contains("Page: Other"), "{again}");

    // A screenshot is saved in the project and handed over as a picture.
    act(json!({ "action": "resize", "width": 500, "height": 400 }))
        .await
        .unwrap();

    let shot = view
        .run_with_images(json!({ "action": "screenshot" }), &context)
        .await
        .unwrap();

    assert!(shot.text.contains(", 500 by 400."), "{}", shot.text);
    assert_eq!(shot.images.len(), 1);
    assert_eq!(shot.images[0].media_type, "image/png");

    let saved: Vec<_> = fs::read_dir(project.path().join(SCREENSHOT_DIRECTORY))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();

    assert_eq!(saved.len(), 1);
    assert_eq!(png_size(&fs::read(&saved[0]).unwrap()), Some((500, 400)));

    let heading = view
        .run_with_images(
            json!({ "action": "screenshot", "selector": "h1" }),
            &context,
        )
        .await
        .unwrap();

    assert!(
        heading.text.contains("Screenshot of <h1> \"Elsewhere\""),
        "{}",
        heading.text
    );

    // Through the registry, as the agent calls it, the picture arrives on
    // the result.
    let mut registry = cortex_core::ToolRegistry::new();

    registry.register(view);

    let result = registry
        .dispatch(
            &cortex_core::ToolCall {
                id: "call_1".to_string(),
                name: "browser_view".to_string(),
                input: json!({ "action": "screenshot" }),
            },
            &context,
        )
        .await;

    assert!(!result.is_error, "{}", result.output);
    assert_eq!(result.images.len(), 1);

    // Only web addresses and the project's own files can be opened.
    let outside = act(json!({ "action": "open", "url": "file:///etc/hosts" }))
        .await
        .unwrap_err();

    assert!(
        outside.to_string().contains("outside the workspace"),
        "{outside}"
    );

    let refused = act(json!({ "action": "open", "url": "127.0.0.1:9" }))
        .await
        .unwrap_err();

    assert!(
        refused
            .to_string()
            .contains("could not open http://127.0.0.1:9"),
        "{refused}"
    );
}

#[tokio::test]
async fn without_pictures_a_screenshot_is_saved_but_not_shown() {
    let Some(project) = project() else { return };

    let context = context(project.path());
    let (browser, view) = tools(BrowserOptions::default());

    assert!(
        view.spec()
            .input_schema
            .to_string()
            .contains("You are not shown it")
    );

    browser
        .run(json!({ "action": "open", "url": "other.html" }), &context)
        .await
        .unwrap();

    let shot = view
        .run_with_images(
            json!({ "action": "screenshot", "full_page": true }),
            &context,
        )
        .await
        .unwrap();

    assert!(shot.images.is_empty());
    assert!(
        shot.text
            .contains("Screenshot of the whole page saved to .cortex/browser/"),
        "{}",
        shot.text
    );
    assert!(shot.text.contains("You are not shown it"), "{}", shot.text);

    // Dropping the tools closes the browser and removes its profile.
    let profile = browser
        .shared
        .session
        .lock()
        .await
        .as_ref()
        .unwrap()
        .profile();

    assert!(profile.is_dir());

    drop((browser, view));

    assert!(!profile.exists());
}

#[test]
fn an_address_is_a_web_address_or_a_file_in_the_project() {
    let project = tempfile::tempdir().unwrap();

    fs::create_dir(project.path().join("my site")).unwrap();
    fs::write(project.path().join("my site/a page.html"), "").unwrap();

    let workspace = Workspace::new(project.path()).unwrap();
    let address = |given: &str| address(given, &workspace);

    assert_eq!(
        address("https://example.com/a?b=1").unwrap(),
        "https://example.com/a?b=1"
    );
    assert_eq!(
        address(" http://localhost:3000 ").unwrap(),
        "http://localhost:3000"
    );
    assert_eq!(
        address("localhost:5173/login").unwrap(),
        "http://localhost:5173/login"
    );
    assert_eq!(address("127.0.0.1:8080").unwrap(), "http://127.0.0.1:8080");
    assert_eq!(
        address("example.com/docs").unwrap(),
        "https://example.com/docs"
    );
    assert_eq!(address("about:blank").unwrap(), "about:blank");

    let file = address("my site/a page.html#top").unwrap();

    assert!(file.starts_with("file:///"), "{file}");
    assert!(file.ends_with("/my%20site/a%20page.html#top"), "{file}");

    for refused in [
        "",
        "javascript:alert(1)",
        "chrome://settings",
        "ftp://example.com/x",
        "file:///etc/passwd",
        "../outside.html",
        "missing page",
    ] {
        assert!(address(refused).is_err(), "{refused:?} was accepted");
    }
}

#[test]
fn what_the_page_logs_is_kept_and_new_trouble_is_counted_once() {
    let (outbox, mut sent) = cdp::Outbox::detached();
    let mut log = PageLog::default();

    let mut record =
        |method: &str, params: Value| log.record(&outbox, method, &params, Some("page"));

    record(
        "Runtime.consoleAPICalled",
        json!({
            "type": "error",
            "args": [{ "type": "string", "value": "boom" }, { "type": "number", "value": 3 },
                     { "type": "object", "description": "Object" }],
            "stackTrace": { "callFrames": [{ "url": "http://localhost/app.js?v=2", "lineNumber": 9 }] },
        }),
    );

    record(
        "Runtime.consoleAPICalled",
        json!({ "type": "log", "args": [{ "value": "hello" }] }),
    );

    record(
        "Network.requestWillBeSent",
        json!({ "requestId": "1", "request": { "url": "http://localhost/api/orders", "method": "POST" } }),
    );
    record(
        "Network.responseReceived",
        json!({ "requestId": "1", "response": { "status": 500 } }),
    );

    record(
        "Network.requestWillBeSent",
        json!({ "requestId": "2", "request": { "url": "http://localhost/ok", "method": "GET" } }),
    );
    record(
        "Network.responseReceived",
        json!({ "requestId": "2", "response": { "status": 200 } }),
    );

    // Given up by the page itself: not a failure.
    record(
        "Network.requestWillBeSent",
        json!({ "requestId": "3", "request": { "url": "http://localhost/dropped", "method": "GET" } }),
    );
    record(
        "Network.loadingFailed",
        json!({ "requestId": "3", "canceled": true, "errorText": "net::ERR_ABORTED" }),
    );

    // Browsers ask for this of every site, and most sites being built
    // have none.
    record(
        "Network.requestWillBeSent",
        json!({ "requestId": "4", "request": { "url": "http://localhost/favicon.ico", "method": "GET" } }),
    );
    record(
        "Network.responseReceived",
        json!({ "requestId": "4", "response": { "status": 404 } }),
    );

    record(
        "Page.javascriptDialogOpening",
        json!({ "type": "alert", "message": "Saved" }),
    );

    assert_eq!(log.console[0], "error: boom 3 Object (app.js:10)");
    assert_eq!(log.console[1], "log: hello");
    assert!(log.console[2].contains("alert box saying \"Saved\""));

    let lines: Vec<String> = log.requests.iter().map(Request::line).collect();

    assert_eq!(
        lines,
        [
            "500 POST http://localhost/api/orders",
            "200 GET http://localhost/ok"
        ]
    );

    let news = log.news().unwrap();

    assert!(news.contains("1 new error in the console"), "{news}");
    assert!(news.contains("1 failed request"), "{news}");
    assert_eq!(log.news(), None);

    // The dialog was answered, on the page it came from.
    drop(outbox);

    let mut written = String::new();

    sent.read_to_string(&mut written).unwrap();

    assert!(written.contains("Page.handleJavaScriptDialog"), "{written}");
    assert!(written.contains("\"sessionId\":\"page\""), "{written}");
}
