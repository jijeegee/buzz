// Minimal ICU MessageFormat parser shared by the language-pack scripts.
//
// Covers what FormatJS and Flutter gen-l10n both accept: plain text,
// {name}, {name, number|date|time[, style]}, {name, plural|selectordinal,
// [offset:n] =n {..} one {..} other {..}}, {name, select, a {..} other {..}},
// `#` inside plural options, and FormatJS rich-text tags <b>..</b> / <br/>.
// Apostrophes are always literal: source strings must not need ICU quoting.

const PLURAL_KINDS = new Set(["plural", "selectordinal"]);
const SIMPLE_KINDS = new Set(["number", "date", "time"]);

export class IcuError extends Error {}

export function parse(input) {
  let pos = 0;

  function fail(message) {
    throw new IcuError(`${message} at ${pos}`);
  }

  function skipSpace() {
    while (pos < input.length && /\s/.test(input[pos])) pos += 1;
  }

  function readName() {
    skipSpace();
    const match = /^[\p{L}\p{N}_-]+/u.exec(input.slice(pos));
    if (!match) fail("expected a name");
    pos += match[0].length;
    skipSpace();
    return match[0];
  }

  function readTagStart() {
    const match = /^<([a-zA-Z][\w-]*)(\/?)>/.exec(input.slice(pos));
    if (!match) return null;
    pos += match[0].length;
    return { name: match[1], selfClosing: match[2] === "/" };
  }

  function parseMessage(inPlural, closingTag) {
    const nodes = [];
    let text = "";
    const flush = () => {
      if (text) nodes.push({ type: "text", value: text });
      text = "";
    };
    while (pos < input.length) {
      const ch = input[pos];
      if (ch === "}") break;
      if (ch === "{") {
        flush();
        nodes.push(parseArgument());
        continue;
      }
      if (ch === "#" && inPlural) {
        flush();
        nodes.push({ type: "pound" });
        pos += 1;
        continue;
      }
      if (ch === "<") {
        const close = /^<\/([a-zA-Z][\w-]*)>/.exec(input.slice(pos));
        if (close) {
          if (close[1] !== closingTag) fail(`unexpected </${close[1]}>`);
          break;
        }
        const tag = readTagStart();
        if (tag) {
          flush();
          if (tag.selfClosing) {
            nodes.push({ type: "tag", name: tag.name, children: null });
          } else {
            const children = parseMessage(inPlural, tag.name);
            const end = `</${tag.name}>`;
            if (!input.startsWith(end, pos)) fail(`missing ${end}`);
            pos += end.length;
            nodes.push({ type: "tag", name: tag.name, children });
          }
          continue;
        }
      }
      text += ch;
      pos += 1;
    }
    flush();
    return nodes;
  }

  function parseArgument() {
    pos += 1; // {
    const name = readName();
    if (input[pos] === "}") {
      pos += 1;
      return { type: "arg", name, kind: "simple" };
    }
    if (input[pos] !== ",") fail("expected , or }");
    pos += 1;
    const kind = readName();
    if (SIMPLE_KINDS.has(kind)) {
      let style = null;
      if (input[pos] === ",") {
        pos += 1;
        const end = input.indexOf("}", pos);
        if (end < 0) fail("unclosed argument");
        style = input.slice(pos, end).trim();
        pos = end;
      }
      if (input[pos] !== "}") fail("expected }");
      pos += 1;
      return { type: "arg", name, kind, style };
    }
    if (!PLURAL_KINDS.has(kind) && kind !== "select")
      fail(`unknown type ${kind}`);
    if (input[pos] !== ",") fail("expected ,");
    pos += 1;
    skipSpace();
    let offset = 0;
    const offsetMatch = /^offset:\s*(\d+)/.exec(input.slice(pos));
    if (offsetMatch) {
      offset = Number(offsetMatch[1]);
      pos += offsetMatch[0].length;
      skipSpace();
    }
    const options = {};
    while (input[pos] !== "}") {
      if (pos >= input.length) fail("unclosed argument");
      const selector = /^(=\d+|[\p{L}\p{N}_-]+)/u.exec(input.slice(pos));
      if (!selector) fail("expected a selector");
      pos += selector[0].length;
      skipSpace();
      if (input[pos] !== "{") fail("expected { after selector");
      pos += 1;
      if (selector[0] in options) fail(`duplicate selector ${selector[0]}`);
      options[selector[0]] = parseMessage(PLURAL_KINDS.has(kind), null);
      if (input[pos] !== "}") fail("unclosed option");
      pos += 1;
      skipSpace();
    }
    pos += 1;
    if (!("other" in options)) fail(`${kind} needs an "other" option`);
    return { type: kind, name, offset, options };
  }

  const nodes = parseMessage(false, null);
  if (pos < input.length) fail("unexpected }");
  return nodes;
}

export function print(nodes) {
  return nodes
    .map((node) => {
      switch (node.type) {
        case "text":
          return node.value;
        case "pound":
          return "#";
        case "tag":
          return node.children === null
            ? `<${node.name}/>`
            : `<${node.name}>${print(node.children)}</${node.name}>`;
        case "arg":
          if (node.kind === "simple") return `{${node.name}}`;
          return node.style
            ? `{${node.name}, ${node.kind}, ${node.style}}`
            : `{${node.name}, ${node.kind}}`;
        default: {
          const offset = node.offset ? ` offset:${node.offset}` : "";
          const options = Object.entries(node.options)
            .map(([sel, child]) => `${sel} {${print(child)}}`)
            .join(" ");
          return `{${node.name}, ${node.type},${offset} ${options}}`;
        }
      }
    })
    .join("");
}

/** Every argument and tag in a message, as sorted "kind:name" strings. */
export function signature(nodes) {
  const out = [];
  const walk = (list) => {
    for (const node of list) {
      if (node.type === "arg") out.push(`${node.kind}:${node.name}`);
      else if (node.type === "tag") {
        out.push(`tag:${node.name}`);
        if (node.children) walk(node.children);
      } else if (
        node.type === "plural" ||
        node.type === "select" ||
        node.type === "selectordinal"
      ) {
        out.push(`${node.type}:${node.name}`);
        for (const child of Object.values(node.options)) walk(child);
      }
    }
  };
  walk(nodes);
  return [...new Set(out)].sort();
}

/** All plural/select nodes in a message, depth first. */
export function choices(nodes) {
  const out = [];
  const walk = (list) => {
    for (const node of list) {
      if (node.type === "tag" && node.children) walk(node.children);
      if (
        node.type === "plural" ||
        node.type === "select" ||
        node.type === "selectordinal"
      ) {
        out.push(node);
        for (const child of Object.values(node.options)) walk(child);
      }
    }
  };
  walk(nodes);
  return out;
}

/** Visible text only (for length and script checks). */
export function visibleText(nodes) {
  return nodes
    .map((node) => {
      if (node.type === "text") return node.value;
      if (node.type === "tag")
        return node.children ? visibleText(node.children) : "";
      if (
        node.type === "plural" ||
        node.type === "select" ||
        node.type === "selectordinal"
      ) {
        return visibleText(node.options.other);
      }
      return "";
    })
    .join("");
}

/** Rewrite every text node; keeps placeholders, tags and selectors intact. */
export function mapText(nodes, fn) {
  return nodes.map((node) => {
    if (node.type === "text") return { ...node, value: fn(node.value) };
    if (node.type === "tag" && node.children)
      return { ...node, children: mapText(node.children, fn) };
    if (
      node.type === "plural" ||
      node.type === "select" ||
      node.type === "selectordinal"
    ) {
      const options = {};
      for (const [sel, child] of Object.entries(node.options))
        options[sel] = mapText(child, fn);
      return { ...node, options };
    }
    return node;
  });
}
