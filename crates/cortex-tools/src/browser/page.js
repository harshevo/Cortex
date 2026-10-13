// Runs inside the page. Evaluates to a function that returns the helper
// object, creating it on first use in each document.
//
// Elements the model can act on are given numbers. A number stays with its
// element for as long as the document lives, and the numbering carries on
// from `start` in a new document, so a number from an earlier page never
// points at something else.
(function (start) {
  if (window.__cortexBrowser) return window.__cortexBrowser;

  const numbers = new WeakMap();
  const elements = new Map();
  let next = start;

  const MAX_NODES = 6000;

  const SKIPPED = new Set([
    "script", "style", "noscript", "template", "head", "meta", "link", "svg", "canvas",
    "video", "audio", "object", "embed", "br", "hr", "wbr",
  ]);

  const CONTROLS =
    "a[href],button,input:not([type=hidden]),select,textarea,summary,[contenteditable=''],[contenteditable='true']," +
    "[role=button],[role=link],[role=checkbox],[role=radio],[role=switch],[role=tab],[role=menuitem]," +
    "[role=menuitemcheckbox],[role=menuitemradio],[role=option],[role=combobox],[role=textbox],[role=slider]";

  const LANDMARKS = {
    nav: "navigation", main: "main", form: "form", dialog: "dialog", table: "table",
    header: "header", footer: "footer", aside: "aside",
  };

  const LANDMARK_ROLES = new Set([
    "dialog", "alertdialog", "alert", "navigation", "main", "form", "search", "menu",
    "tablist", "tabpanel", "listbox", "status", "banner", "region",
  ]);

  const clean = (text) => (text || "").replace(/\s+/g, " ").trim();

  const cut = (text, most) => (text.length > most ? text.slice(0, most - 1) + "…" : text);

  const quote = (text) => JSON.stringify(text);

  function numberOf(element) {
    let number = numbers.get(element);

    if (!number) {
      number = next++;
      numbers.set(element, number);
      elements.set(number, new WeakRef(element));
    }

    return number;
  }

  function roleOf(element) {
    const role = element.getAttribute("role");

    if (role) return role;

    switch (element.localName) {
      case "a": return "link";
      case "button": case "summary": return "button";
      case "select": return "combobox";
      case "textarea": return "textbox";
      case "input":
        switch (element.type) {
          case "checkbox": return "checkbox";
          case "radio": return "radio";
          case "range": return "slider";
          case "file": return "file chooser";
          case "submit": case "button": case "reset": case "image": return "button";
          default: return "textbox";
        }
    }

    return element.isContentEditable ? "textbox" : "clickable";
  }

  function nameOf(element) {
    const label = element.getAttribute("aria-label");

    if (label) return clean(label);

    const labelledBy = element.getAttribute("aria-labelledby");

    if (labelledBy) {
      const text = labelledBy
        .split(/\s+/)
        .map((id) => document.getElementById(id))
        .filter(Boolean)
        .map((labelling) => labelling.innerText)
        .join(" ");

      if (clean(text)) return clean(text);
    }

    const tag = element.localName;

    if (tag === "input" || tag === "select" || tag === "textarea") {
      const own = element.labels && element.labels[0];

      if (own) {
        // A label that wraps its field would otherwise bring the field's
        // own contents, such as a list's options, into the name.
        const words = own.cloneNode(true);

        words.querySelectorAll("input,select,textarea,button").forEach((control) => control.remove());

        if (clean(words.textContent)) return clean(words.textContent);
      }

      if (tag === "input" && ["submit", "button", "reset"].includes(element.type)) {
        return clean(element.value);
      }

      return clean(element.placeholder || element.title || element.name || "");
    }

    const text = clean(element.innerText);

    if (text) return text;

    const picture = element.querySelector("img[alt]");

    return clean((picture && picture.alt) || element.title || "");
  }

  function statesOf(element, role) {
    const states = [];
    const tag = element.localName;
    const attribute = (name) => element.getAttribute(name);

    if (tag === "a") {
      let address = element.getAttribute("href") || "";

      try {
        const parsed = new URL(element.href);

        if (parsed.origin === location.origin) address = parsed.pathname + parsed.search + parsed.hash;
      } catch (_) {}

      if (address) states.push("→ " + cut(address, 90));
    }

    if (tag === "input" && !["checkbox", "radio", "submit", "button", "reset", "file"].includes(element.type)) {
      if (element.type !== "text") states.push("type=" + element.type);

      // What a password field holds is nobody's business but the page's.
      if (element.value) {
        states.push("value=" + (element.type === "password" ? quote("•".repeat(Math.min(element.value.length, 12))) : quote(cut(element.value, 80))));
      }
    }

    if (tag === "textarea" && element.value) states.push("value=" + quote(cut(clean(element.value), 80)));

    if (tag === "select") {
      const chosen = [...element.selectedOptions].map((option) => clean(option.label)).join(", ");

      states.push("value=" + quote(chosen));

      const options = [...element.options].map((option) => clean(option.label));

      states.push("options: " + cut(options.slice(0, 15).join(" | "), 200) + (options.length > 15 ? " | …" : ""));
    }

    if (element.isContentEditable && tag !== "input" && tag !== "textarea") states.push("editable");

    if (role === "checkbox" || role === "radio" || role === "switch") {
      const checked = tag === "input" ? element.checked : attribute("aria-checked") === "true";

      states.push(checked ? "checked" : "unchecked");
    }

    if (element.disabled || attribute("aria-disabled") === "true") states.push("disabled");
    if (attribute("aria-expanded")) states.push(attribute("aria-expanded") === "true" ? "expanded" : "collapsed");
    if (attribute("aria-selected") === "true" || attribute("aria-current")) states.push("selected");
    if (attribute("aria-pressed") === "true") states.push("pressed");
    if (element.required || attribute("aria-required") === "true") states.push("required");
    if (attribute("aria-invalid") === "true") states.push("invalid");
    if (element === document.activeElement) states.push("focused");

    return states.length ? " " + states.join(", ") : "";
  }

  function describe(element) {
    if (!element || element.nodeType !== 1) return "the page";

    const name = cut(nameOf(element), 60);
    const role = roleOf(element);
    const kind = role === "clickable" ? "<" + element.localName + ">" : role;

    return name ? kind + " " + quote(name) : kind;
  }

  function query(selector) {
    let found;

    try {
      found = document.querySelector(selector);
    } catch (_) {
      throw new Error("`" + selector + "` is not a valid CSS selector");
    }

    if (!found) throw new Error("nothing on the page matches the selector `" + selector + "`");

    return found;
  }

  function resolve(target) {
    if (target.selector) return query(target.selector);

    const reference = elements.get(target.ref);
    const element = reference && reference.deref();

    if (!element || !element.isConnected) {
      throw new Error(
        "there is no element [" + target.ref + "] on the page now; the page has changed, so take a new snapshot"
      );
    }

    return element;
  }

  function hidden(element, style) {
    return (
      style.display === "none" ||
      style.visibility === "hidden" ||
      style.visibility === "collapse" ||
      element.hidden ||
      element.getAttribute("aria-hidden") === "true"
    );
  }

  // The lines of a snapshot: one per control, heading, table row and run
  // of text, indented under the part of the page it belongs to.
  function snapshot(options) {
    const root = options.selector ? query(options.selector) : document.body;

    const lines = [];
    let pending = "";
    let depth = 0;
    let visited = 0;
    let cutShort = false;

    // Above zero inside a label, whose words are already its field's name.
    let muted = 0;

    const pad = () => "  ".repeat(depth);

    const flush = () => {
      const text = clean(pending);

      pending = "";

      if (text) lines.push(pad() + "text " + quote(cut(text, 400)));
    };

    const emit = (line) => {
      flush();
      lines.push(pad() + line);
    };

    function walk(node) {
      if (node.nodeType === Node.TEXT_NODE) {
        if (!muted) pending += node.nodeValue;
        return;
      }

      if (node.nodeType !== Node.ELEMENT_NODE) return;

      const element = node;
      const tag = element.localName;

      if (SKIPPED.has(tag)) return;

      if (++visited > MAX_NODES) {
        cutShort = true;
        return;
      }

      const style = getComputedStyle(element);

      if (hidden(element, style)) return;

      const block = !style.display.startsWith("inline") && style.display !== "contents";

      if (block) flush();

      if (tag === "label" && element.control) {
        flush();
        muted++;
        children(element);
        muted--;

        return;
      }

      const control = element.matches(CONTROLS);

      // Something made clickable without saying what it is: a handler, or
      // a pointer cursor its parent does not already have.
      const clickable =
        !control &&
        (element.hasAttribute("onclick") ||
          (element.tabIndex >= 0 && element.hasAttribute("tabindex")) ||
          (style.cursor === "pointer" &&
            !(element.parentElement && getComputedStyle(element.parentElement).cursor === "pointer")));

      if (control || clickable) {
        const role = roleOf(element);
        const inner = clickable && element.querySelector(CONTROLS);

        // A clickable region with controls of its own is shown as a
        // region, with those controls beneath it.
        const name = cut(nameOf(element), inner ? 50 : 100);

        emit("[" + numberOf(element) + "] " + role + (name ? " " + quote(name) : "") + statesOf(element, role));

        if (!inner) return;

        depth++;
        pending = "";
        children(element);
        pending = "";
        depth--;

        return;
      }

      if (/^h[1-6]$/.test(tag) && !element.querySelector(CONTROLS)) {
        const text = clean(element.innerText);

        if (text) emit("heading " + tag[1] + " " + quote(cut(text, 200)));

        return;
      }

      if (tag === "img") {
        if (clean(element.alt)) emit("image " + quote(cut(clean(element.alt), 120)));

        return;
      }

      if (tag === "iframe") {
        emit("iframe " + cut(element.src || "(inline)", 120) + " (its contents are not listed)");

        return;
      }

      if (tag === "tr" && !element.querySelector(CONTROLS)) {
        const cells = [...element.cells].map((cell) => cut(clean(cell.innerText), 80));

        if (cells.some(Boolean)) emit("row " + cells.join(" | "));

        return;
      }

      const role = element.getAttribute("role");
      const landmark = LANDMARKS[tag] || (LANDMARK_ROLES.has(role) ? role : null);

      if (landmark) {
        const label = clean(element.getAttribute("aria-label") || "");

        emit(landmark + (label ? " " + quote(label) : ""));
        depth++;
      }

      children(element);

      if (block) flush();

      if (landmark) {
        flush();
        depth--;
      }
    }

    function children(element) {
      let nodes = element.shadowRoot ? element.shadowRoot.childNodes : element.childNodes;

      if (element.localName === "slot") {
        const assigned = element.assignedNodes({ flatten: true });

        if (assigned.length) nodes = assigned;
      }

      for (const child of nodes) walk(child);
    }

    walk(root);
    flush();

    return { lines, cutShort, next, page: page() };
  }

  function page() {
    const scroller = document.scrollingElement || document.documentElement;

    return {
      url: location.href,
      title: document.title,
      width: window.innerWidth,
      height: window.innerHeight,
      scrolled: Math.round(window.scrollY),
      tall: scroller ? scroller.scrollHeight : 0,
      next,
    };
  }

  // Brings an element into view and says where to click it. Refuses when
  // a click there would land on something else, as it would for a person.
  function point(target, forClick) {
    const element = resolve(target);

    element.scrollIntoView({ block: "center", inline: "center", behavior: "instant" });

    const box = element.getBoundingClientRect();

    if (box.width === 0 && box.height === 0) {
      throw new Error(describe(element) + " has no size on the page, so it cannot be seen or clicked");
    }

    if (forClick && (element.disabled || element.getAttribute("aria-disabled") === "true")) {
      throw new Error(describe(element) + " is disabled");
    }

    const x = Math.min(Math.max(box.left + box.width / 2, 1), window.innerWidth - 1);
    const y = Math.min(Math.max(box.top + box.height / 2, 1), window.innerHeight - 1);

    let top = document.elementFromPoint(x, y);

    while (top && top.shadowRoot) {
      const deeper = top.shadowRoot.elementFromPoint(x, y);

      if (!deeper || deeper === top) break;

      top = deeper;
    }

    const related = (other) => other === element || element.contains(other) || other.contains(element);

    // A styled checkbox is often clicked through its label.
    const ownLabel = (other) => element.labels && [...element.labels].some((label) => label.contains(other));

    if (forClick && top && !related(top) && !ownLabel(top)) {
      throw new Error(describe(element) + " is covered by " + describe(top) + ", so a click would not reach it");
    }

    if (forClick) {
      // There is one page here, so a link that would open another tab
      // opens in this one.
      const link = element.closest("a[target]");

      if (link && link.target !== "_self") link.target = "_self";
    }

    return { x, y, what: describe(element), left: box.left + window.scrollX, top: box.top + window.scrollY, width: box.width, height: box.height };
  }

  // Puts the cursor in a field with what it holds selected, so that typing
  // replaces it.
  function focus(target, keep) {
    const element = resolve(target);
    const tag = element.localName;

    const field = tag === "input" || tag === "textarea";

    if (!field && !element.isContentEditable) {
      throw new Error(describe(element) + " is not something that can be typed into");
    }

    if (element.disabled || element.readOnly) throw new Error(describe(element) + " cannot be edited");

    element.scrollIntoView({ block: "center", inline: "center", behavior: "instant" });
    element.focus();

    let filled = false;

    if (field) {
      filled = element.value !== "";

      try {
        if (keep) element.setSelectionRange(element.value.length, element.value.length);
        else element.select();
      } catch (_) {
        // Some kinds of field (date, number, email) have no selection.
        // Emptying one directly is the next best thing.
        if (!keep && filled) {
          const setter = Object.getOwnPropertyDescriptor(Object.getPrototypeOf(element), "value").set;

          setter.call(element, "");
          element.dispatchEvent(new Event("input", { bubbles: true }));
          filled = false;
        }
      }
    } else {
      filled = clean(element.innerText) !== "";

      const range = document.createRange();

      range.selectNodeContents(element);

      if (keep) range.collapse(false);

      const selection = window.getSelection();

      selection.removeAllRanges();
      selection.addRange(range);
    }

    return { what: describe(element), filled: filled && !keep };
  }

  function choose(target, value) {
    const element = resolve(target);

    if (element.localName !== "select") {
      throw new Error(describe(element) + " is not a drop-down list; click it and then click the option");
    }

    if (element.disabled) throw new Error(describe(element) + " is disabled");

    const wanted = clean(value).toLowerCase();
    const options = [...element.options];

    const option =
      options.find((candidate) => candidate.value === value) ||
      options.find((candidate) => clean(candidate.label).toLowerCase() === wanted) ||
      options.find((candidate) => clean(candidate.label).toLowerCase().includes(wanted));

    if (!option) {
      throw new Error(
        "no option " + quote(value) + "; the options are: " + options.map((candidate) => clean(candidate.label)).join(" | ")
      );
    }

    element.value = option.value;
    element.dispatchEvent(new Event("input", { bubbles: true }));
    element.dispatchEvent(new Event("change", { bubbles: true }));

    return { what: describe(element), chose: clean(option.label) };
  }

  function scroll(target, direction) {
    if (target) {
      const element = resolve(target);

      element.scrollIntoView({ block: "center", inline: "center", behavior: "instant" });

      return page();
    }

    const step = Math.round(window.innerHeight * 0.85);

    switch (direction) {
      case "up": window.scrollBy(0, -step); break;
      case "top": window.scrollTo(0, 0); break;
      case "bottom": window.scrollTo(0, (document.scrollingElement || document.documentElement).scrollHeight); break;
      default: window.scrollBy(0, step);
    }

    return page();
  }

  // Whether what a `wait` is waiting for is there.
  function present(what) {
    if (what.selector) {
      let found;

      try {
        found = document.querySelector(what.selector);
      } catch (_) {
        throw new Error("`" + what.selector + "` is not a valid CSS selector");
      }

      if (!found) return false;

      return !hidden(found, getComputedStyle(found)) && found.getClientRects().length > 0;
    }

    return document.body ? document.body.innerText.includes(what.text) : false;
  }

  window.__cortexBrowser = { snapshot, page, point, focus, choose, scroll, present };

  return window.__cortexBrowser;
})
