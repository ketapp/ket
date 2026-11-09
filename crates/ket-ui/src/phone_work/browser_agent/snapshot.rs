//! `ket browser snapshot`: the page's accessibility tree, a node a line,
//! each with a ref the actions take.
//!
//! The tree is Chromium's own (`Accessibility.getFullAXTree`), not one
//! rebuilt from the DOM: its roles, names and states are what a screen
//! reader is given — a `<div role=button>` is a button, a label names its
//! field, `aria-hidden` and `display: none` are gone — which is what an agent
//! needs to find its way round a page. `DOMSnapshot` has the layout but
//! none of that. What a tree holds that nobody reads — layout wrappers, text
//! a button's name already says, the text boxes inside each line — is left
//! out; their children stand in their place. Each line with a DOM node of
//! its own gets a ref — see `refs`.
//!
//! A page's frames have trees of their own. A frame from the page's own site
//! runs in the page's process, where the protocol reaches it: its tree is
//! asked for by its frame (`frameId`) and shown under its `<iframe>`'s line,
//! its nodes' refs naming the frame they are in. A frame from another site
//! runs in a process of its own — Chromium's site isolation — that this
//! page's protocol does not reach: its line says `cross-site=` and where it is
//! from, with nothing under it.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use gpui::FutureExt as _;
use ket_chromium::protocol::browser_protocol::accessibility::{
    AxNode, AxPropertyName, AxValue, DisableParams, GetFullAxTreeParams,
};
use ket_chromium::protocol::browser_protocol::dom::{
    BackendNodeId, DescribeNodeParams, GetFrameOwnerParams,
};
use ket_chromium::protocol::browser_protocol::page::{FrameId, FrameTree, GetFrameTreeParams};
use ket_core::host::BrowserAnswer;
use ket_core::host::wire::SnapshotNode;
use serde_json::Value;

use super::refs::with_refs;
use super::{Answer, LOOK_WAIT, Page};

/// How long a page may take to give its accessibility tree: a big one is
/// built on the spot.
const TREE_WAIT: Duration = Duration::from_secs(10);

/// The longest name shown, in characters.
const NAME_MAX: usize = 100;

/// The longest run of text shown, in characters.
const TEXT_MAX: usize = 200;

/// How far down the tree is followed: past it a page is nested beyond what
/// a line can show.
const LEVELS_MAX: usize = 256;

/// The most frames a snapshot looks into, and the most frames from other
/// sites it says where they are from.
const FRAMES_MAX: usize = 20;

/// How long a snapshot's frames may take between them.
const FRAMES_WAIT: Duration = Duration::from_secs(5);

/// Roles an agent acts on: what `--interactive` keeps.
const ACTIONABLE: &[&str] = &[
    "button",
    "checkbox",
    "combobox",
    "link",
    "listbox",
    "menuitem",
    "menuitemcheckbox",
    "menuitemradio",
    "option",
    "radio",
    "scrollbar",
    "searchbox",
    "slider",
    "spinbutton",
    "switch",
    "tab",
    "textbox",
    "treeitem",
    // Chromium's own names for what has no ARIA one.
    "DisclosureTriangle",
    "MenuListOption",
    "PopUpButton",
    "ToggleButton",
];

/// Roles whose value — what a field holds — is shown.
const FIELDS: &[&str] = &[
    "combobox",
    "meter",
    "progressbar",
    "searchbox",
    "slider",
    "spinbutton",
    "textbox",
];

/// Roles that only hold what is inside them: left out when they have no
/// name, their children shown in their place.
const WRAPPERS: &[&str] = &[
    "generic",
    "group",
    "none",
    "presentation",
    "LabelText",
    "Section",
];

impl Page<'_> {
    /// The page's accessibility tree, at most `max` lines of it, each node
    /// with a ref — only what can be acted on, if `interactive`.
    pub(super) async fn snapshot(&self, interactive: bool, max: u32) -> Answer {
        let spot = self
            .spot()
            .await
            .ok_or_else(|| "the page didn't say where it is".to_owned())?;
        let tree = self
            .cdp
            .call(GetFullAxTreeParams::default())
            .with_timeout(TREE_WAIT, self.executor)
            .await
            .map_err(|_| "the page didn't give its accessibility tree in time".to_owned())?
            .map_err(|error| error.to_string())?;
        let mut trees = vec![Tree {
            frame: spot.frame.clone(),
            owner: None,
            nodes: tree.nodes,
        }];
        // Short of how long the window gives the whole snapshot.
        let by = Instant::now() + FRAMES_WAIT;
        trees.extend(self.frames(by).await);
        // Asking builds the tree and keeps it built, kept up to date on
        // every change to the page, until this: a cost the person browsing
        // it would pay.
        let _ = self
            .cdp
            .call(DisableParams::default())
            .with_timeout(LOOK_WAIT, self.executor)
            .await;
        let away = self.elsewhere(&trees, by).await;
        let lines = lines(&trees, &away, interactive);
        let max = max as usize;
        let cut = u32::try_from(lines.len().saturating_sub(max)).unwrap_or(u32::MAX);
        let nodes = with_refs(self.refs, |refs| {
            refs.on(&spot.loader);
            lines
                .into_iter()
                .take(max)
                .map(|line| SnapshotNode {
                    r#ref: line
                        .node
                        .and_then(|node| refs.of(&trees[line.frame].frame, node)),
                    depth: line.depth,
                    role: line.role,
                    name: line.name,
                    value: line.value,
                    states: line.states,
                })
                .collect()
        });
        let (url, title) = self.address(Some(spot)).await?;
        Ok(BrowserAnswer::Snapshot {
            url,
            title,
            nodes,
            cut,
        })
    }

    /// The trees of the page's frames that run in its process, up to
    /// [`FRAMES_MAX`], each with the `<iframe>` it is in, as many as come by
    /// `by`. One that doesn't give its tree in time is empty.
    async fn frames(&self, by: Instant) -> Vec<Tree> {
        let Some(tree) = self.by(GetFrameTreeParams::default(), by).await else {
            return Vec::new();
        };
        let mut trees = Vec::new();
        for frame in frames_in(tree.frame_tree).into_iter().take(FRAMES_MAX) {
            let id = || FrameId::new(frame.clone());
            let Some(owner) = self.by(GetFrameOwnerParams::new(id()), by).await else {
                continue;
            };
            let asked = GetFullAxTreeParams {
                frame_id: Some(id()),
                ..GetFullAxTreeParams::default()
            };
            let nodes = self.by(asked, by).await.map(|tree| tree.nodes);
            trees.push(Tree {
                frame,
                owner: Some(*owner.backend_node_id.inner()),
                nodes: nodes.unwrap_or_default(),
            });
        }
        trees
    }

    /// The `<iframe>`s in `trees` whose frame runs in a process of its own —
    /// another site's — by backend DOM id, each with the host its address
    /// names, up to [`FRAMES_MAX`], as many as are told by `by`.
    async fn elsewhere(&self, trees: &[Tree], by: Instant) -> HashMap<i64, String> {
        let here: HashSet<i64> = trees.iter().filter_map(|tree| tree.owner).collect();
        let frames = trees
            .iter()
            .flat_map(|tree| &tree.nodes)
            .filter(|node| !node.ignored && is_frame(node))
            .filter_map(backend)
            .filter(|id| !here.contains(id));
        let mut away = HashMap::new();
        for id in frames.take(FRAMES_MAX) {
            let asked = DescribeNodeParams {
                backend_node_id: Some(BackendNodeId::new(id)),
                ..DescribeNodeParams::default()
            };
            let attributes = self.by(asked, by).await.and_then(|it| it.node.attributes);
            let source = attribute(attributes.as_deref(), "src");
            away.insert(id, crate::chromium::host(source).to_owned());
        }
        away
    }

    /// The answer to `command`, if it comes by `by`.
    async fn by<C: ket_chromium::Command>(&self, command: C, by: Instant) -> Option<C::Response> {
        let left = by.checked_duration_since(Instant::now())?;
        self.cdp
            .call(command)
            .with_timeout(left, self.executor)
            .await
            .ok()?
            .ok()
    }
}

/// The ids of the frames under `tree`'s, depth first, not its own.
pub(super) fn frames_in(tree: FrameTree) -> Vec<String> {
    let mut frames = Vec::new();
    let mut left: Vec<FrameTree> = tree.child_frames.unwrap_or_default();
    left.reverse();
    while let Some(next) = left.pop() {
        frames.push(next.frame.id.inner().clone());
        let mut children = next.child_frames.unwrap_or_default();
        children.reverse();
        left.extend(children);
    }
    frames
}

/// The value of attribute `name` in a DOM node's `attributes`, which the
/// protocol gives as names and values in turn; empty when it has none.
fn attribute<'a>(attributes: Option<&'a [String]>, name: &str) -> &'a str {
    attributes
        .unwrap_or_default()
        .as_chunks::<2>()
        .0
        .iter()
        .find(|[key, _]| key == name)
        .map_or("", |[_, value]| value.as_str())
}

/// Whether `node` is an `<iframe>` or `<frame>`.
fn is_frame(node: &AxNode) -> bool {
    text(node.role.as_ref()).starts_with("Iframe")
}

/// One document's accessibility tree: the page's, or a frame's in it.
pub(super) struct Tree {
    /// The frame whose document it is.
    pub frame: String,
    /// The `<iframe>` the frame is in, by backend DOM id; `None` for the
    /// page's own.
    pub owner: Option<i64>,
    /// Its nodes, as Chromium gives them.
    pub nodes: Vec<AxNode>,
}

/// A node of the tree as a line, before it has a ref.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Line {
    /// How far in it is under the lines above it.
    pub depth: u16,
    /// Its role, `text` for text on its own.
    pub role: String,
    /// Its name, or the text.
    pub name: String,
    /// What a field holds.
    pub value: Option<String>,
    /// Its states, a word each.
    pub states: Vec<String>,
    /// Its backend DOM id, if it has a DOM node.
    pub node: Option<i64>,
    /// Which of the trees it came from, whose frame its node is in.
    pub frame: usize,
}

/// A line and the lines under it.
struct Item {
    line: Line,
    children: Vec<Item>,
}

/// The lines of `trees` — the page's first, then its frames' — in document
/// order, each frame's under its `<iframe>`: every node anyone would read,
/// or only what can be acted on. `away` names the frames from other sites,
/// whose trees nothing here has.
pub(super) fn lines(trees: &[Tree], away: &HashMap<i64, String>, interactive: bool) -> Vec<Line> {
    let mut walk = Walk {
        by_id: trees
            .iter()
            .map(|tree| {
                tree.nodes
                    .iter()
                    .map(|node| (node.node_id.as_ref(), node))
                    .collect()
            })
            .collect(),
        trees,
        framed: trees
            .iter()
            .enumerate()
            .filter_map(|(at, tree)| Some((tree.owner?, at)))
            .collect(),
        away,
        interactive,
        seen: HashSet::new(),
    };
    let items = walk.tree(0, "", 0);
    let mut lines = Vec::new();
    flatten(items, 0, &mut lines);
    lines
}

/// A walk down the trees.
struct Walk<'a> {
    trees: &'a [Tree],
    /// Each tree's nodes by id, which are only unique within a tree.
    by_id: Vec<HashMap<&'a str, &'a AxNode>>,
    /// Which tree is in each `<iframe>`, by its backend DOM id.
    framed: HashMap<i64, usize>,
    away: &'a HashMap<i64, String>,
    interactive: bool,
    /// The nodes walked, by tree, so a tree that loops ends.
    seen: HashSet<(usize, &'a str)>,
}

impl<'a> Walk<'a> {
    /// What tree `at` is shown as, where the nearest line above has name
    /// `above`.
    fn tree(&mut self, at: usize, above: &str, level: usize) -> Vec<Item> {
        let Some(tree) = self.trees.get(at) else {
            return Vec::new();
        };
        let nodes = &tree.nodes;
        match nodes
            .iter()
            .find(|node| node.parent_id.is_none())
            .or(nodes.first())
        {
            Some(root) => self.items(at, root, above, level),
            None => Vec::new(),
        }
    }

    /// What `node`, in tree `at`, is shown as — itself, or what is under it
    /// in its place — where the nearest line above has name `above`.
    fn items(&mut self, at: usize, node: &'a AxNode, above: &str, level: usize) -> Vec<Item> {
        if level > LEVELS_MAX || !self.seen.insert((at, node.node_id.as_ref())) {
            return Vec::new();
        }
        let role = text(node.role.as_ref());
        let mut name = squash(&text(node.name.as_ref()));
        if !node.ignored {
            match role.as_str() {
                // The pieces of a line of text, a list's bullet, a line
                // break: the text says them.
                "InlineTextBox" | "ListMarker" | "LineBreak" => return Vec::new(),
                "StaticText" => {
                    let said = name.is_empty()
                        || name.chars().all(|c| c.is_ascii_punctuation())
                        || above.contains(name.as_str());
                    if self.interactive || said {
                        return Vec::new();
                    }
                    return vec![Item {
                        line: Line {
                            depth: 0,
                            role: "text".to_owned(),
                            name: cut(name, TEXT_MAX),
                            value: None,
                            states: Vec::new(),
                            node: backend(node),
                            frame: at,
                        },
                        children: Vec::new(),
                    }];
                }
                _ => {}
            }
        }
        let mut states = states(node, &role);
        // Another site's frame: what is in it is out of reach, but a click
        // on it lands in it.
        let away = backend(node).and_then(|id| self.away.get(&id));
        if let Some(host) = away {
            states.push(match host.as_str() {
                "" => "cross-site".to_owned(),
                host => format!("cross-site={host}"),
            });
        }
        let actionable = away.is_some()
            || ACTIONABLE.contains(&role.as_str())
            || (role != "RootWebArea"
                && node.properties.iter().flatten().any(|property| {
                    property.name == AxPropertyName::Editable
                        && property.value.value.as_ref().and_then(Value::as_str) != Some("")
                }));
        if name.is_empty() && actionable {
            name = squash(&text(node.description.as_ref()));
        }
        let shown = !node.ignored
            && role != "RootWebArea"
            && if self.interactive {
                actionable
            } else {
                !(name.is_empty() && WRAPPERS.contains(&role.as_str()))
            };
        let kids: Vec<&'a AxNode> = node
            .child_ids
            .iter()
            .flatten()
            .filter_map(|id| self.by_id[at].get(id.as_ref()).copied())
            .collect();
        let below = if shown && !name.is_empty() {
            name.as_str()
        } else {
            above
        };
        let mut children = Vec::new();
        for kid in kids {
            children.extend(self.items(at, kid, below, level + 1));
        }
        // An `<iframe>` from the page's site holds its frame's tree.
        if let Some(&inner) = backend(node).and_then(|id| self.framed.get(&id))
            && !node.ignored
            && inner != at
        {
            children.extend(self.tree(inner, below, level + 1));
        }
        // Text beside a field it names — a checkbox's label — says nothing
        // the field does not.
        let named: HashSet<String> = children
            .iter()
            .filter(|item| item.line.role != "text")
            .map(|item| item.line.name.clone())
            .collect();
        children.retain(|item| {
            !(item.line.role == "text"
                && item.children.is_empty()
                && named.contains(&item.line.name))
        });
        if !shown {
            return children;
        }
        let value = FIELDS
            .contains(&role.as_str())
            .then(|| squash(&text(node.value.as_ref())))
            .filter(|value| !value.is_empty() && *value != name)
            .map(|value| cut(value, NAME_MAX));
        let mut item = Item {
            line: Line {
                depth: 0,
                role: if role.is_empty() {
                    "node".to_owned()
                } else {
                    role
                },
                name: cut(name, NAME_MAX),
                value,
                states,
                node: backend(node),
                frame: at,
            },
            children,
        };
        // A paragraph, a list item, a cell with nothing in it but text: the
        // text on its line.
        if item.line.name.is_empty()
            && let [only] = item.children.as_slice()
            && only.line.role == "text"
            && only.children.is_empty()
        {
            item.line.name = only.line.name.clone();
            item.children.clear();
        }
        vec![item]
    }
}

/// `items`' lines, depth first, each `depth` in or further.
fn flatten(items: Vec<Item>, depth: u16, lines: &mut Vec<Line>) {
    for item in items {
        let mut line = item.line;
        line.depth = depth;
        lines.push(line);
        flatten(item.children, depth.saturating_add(1), lines);
    }
}

/// A node's states, a word each, in the order Chromium lists them: what
/// `role` is in, and a heading's level.
fn states(node: &AxNode, role: &str) -> Vec<String> {
    let mut states = Vec::new();
    for property in node.properties.iter().flatten() {
        let value = property.value.value.as_ref();
        let word = value.and_then(Value::as_str);
        let on = value.and_then(Value::as_bool) == Some(true) || word == Some("true");
        let state = match property.name {
            AxPropertyName::Checked | AxPropertyName::Pressed if word == Some("mixed") => "mixed",
            AxPropertyName::Checked if on => "checked",
            AxPropertyName::Pressed if on => "pressed",
            AxPropertyName::Selected if on => "selected",
            AxPropertyName::Expanded if on => "expanded",
            AxPropertyName::Expanded => "collapsed",
            AxPropertyName::Disabled if on => "disabled",
            AxPropertyName::Focused if on => "focused",
            AxPropertyName::Required if on => "required",
            AxPropertyName::Readonly if on => "readonly",
            AxPropertyName::Modal if on => "modal",
            AxPropertyName::Invalid if word.is_some_and(|word| word != "false") => "invalid",
            AxPropertyName::Level if role == "heading" => {
                if let Some(level) = value.and_then(Value::as_f64) {
                    states.push(format!("level={level}"));
                }
                continue;
            }
            _ => continue,
        };
        states.push(state.to_owned());
    }
    states
}

/// What an accessibility value says, as text: empty when it says nothing.
fn text(value: Option<&AxValue>) -> String {
    match value.and_then(|value| value.value.as_ref()) {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Number(number)) => number.to_string(),
        _ => String::new(),
    }
}

/// A node's backend DOM id, if it has a DOM node.
fn backend(node: &AxNode) -> Option<i64> {
    node.backend_dom_node_id.as_ref().map(|id| *id.inner())
}

/// `text` with each run of white space one space, and none at the ends.
fn squash(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `text` cut to `max` characters, ending in an ellipsis where it was cut.
fn cut(text: String, max: usize) -> String {
    if text.chars().count() <= max {
        return text;
    }
    let mut cut: String = text.chars().take(max.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

#[cfg(test)]
mod tests {
    use super::*;
    use ket_chromium::protocol::browser_protocol::accessibility::GetFullAxTreeReturns;

    /// A sign-in page's tree, as Chromium gives it.
    const SIGN_IN: &str = r#"{"nodes": [
        {"nodeId": "1", "ignored": false, "role": {"type": "internalRole", "value": "RootWebArea"}, "name": {"type": "computedString", "value": "Sign in"}, "childIds": ["2"], "backendDOMNodeId": 1},
        {"nodeId": "2", "ignored": false, "role": {"type": "role", "value": "generic"}, "parentId": "1", "childIds": ["3", "4", "10", "15", "20", "22", "25"], "backendDOMNodeId": 2},
        {"nodeId": "3", "ignored": false, "role": {"type": "role", "value": "heading"}, "parentId": "2", "name": {"type": "computedString", "value": "Sign  in\n"}, "properties": [{"name": "level", "value": {"type": "integer", "value": 1}}], "childIds": ["30"], "backendDOMNodeId": 3},
        {"nodeId": "30", "ignored": false, "role": {"type": "internalRole", "value": "StaticText"}, "parentId": "3", "name": {"type": "computedString", "value": "Sign in"}, "childIds": ["31"], "backendDOMNodeId": 30},
        {"nodeId": "31", "ignored": false, "role": {"type": "internalRole", "value": "InlineTextBox"}, "parentId": "30", "name": {"type": "computedString", "value": "Sign in"}},
        {"nodeId": "4", "ignored": true, "ignoredReasons": [{"name": "uninteresting", "value": {"type": "boolean", "value": true}}], "role": {"type": "role", "value": "none"}, "parentId": "2", "childIds": ["5"], "backendDOMNodeId": 4},
        {"nodeId": "5", "ignored": false, "role": {"type": "role", "value": "textbox"}, "parentId": "4", "name": {"type": "computedString", "value": "Email"}, "value": {"type": "string", "value": "brad@example.com"}, "backendDOMNodeId": 105, "properties": [
            {"name": "focusable", "value": {"type": "booleanOrUndefined", "value": true}},
            {"name": "focused", "value": {"type": "booleanOrUndefined", "value": true}},
            {"name": "editable", "value": {"type": "token", "value": "plaintext"}},
            {"name": "required", "value": {"type": "boolean", "value": true}},
            {"name": "invalid", "value": {"type": "token", "value": "false"}}]},
        {"nodeId": "10", "ignored": false, "role": {"type": "internalRole", "value": "LabelText"}, "parentId": "2", "childIds": ["11", "12"], "backendDOMNodeId": 10},
        {"nodeId": "11", "ignored": false, "role": {"type": "role", "value": "checkbox"}, "parentId": "10", "name": {"type": "computedString", "value": "Remember me"}, "properties": [{"name": "checked", "value": {"type": "tristate", "value": "true"}}], "backendDOMNodeId": 111},
        {"nodeId": "12", "ignored": false, "role": {"type": "internalRole", "value": "StaticText"}, "parentId": "10", "name": {"type": "computedString", "value": "Remember me"}, "backendDOMNodeId": 112},
        {"nodeId": "15", "ignored": false, "role": {"type": "role", "value": "paragraph"}, "parentId": "2", "childIds": ["16", "17", "18"], "backendDOMNodeId": 15},
        {"nodeId": "16", "ignored": false, "role": {"type": "internalRole", "value": "StaticText"}, "parentId": "15", "name": {"type": "computedString", "value": "New here? "}, "backendDOMNodeId": 116},
        {"nodeId": "17", "ignored": false, "role": {"type": "role", "value": "link"}, "parentId": "15", "name": {"type": "computedString", "value": "Create an account"}, "value": {"type": "string", "value": "http://localhost:3000/join"}, "childIds": ["19"], "backendDOMNodeId": 117},
        {"nodeId": "19", "ignored": false, "role": {"type": "internalRole", "value": "StaticText"}, "parentId": "17", "name": {"type": "computedString", "value": "Create an account"}, "backendDOMNodeId": 119},
        {"nodeId": "18", "ignored": false, "role": {"type": "internalRole", "value": "StaticText"}, "parentId": "15", "name": {"type": "computedString", "value": "."}, "backendDOMNodeId": 118},
        {"nodeId": "20", "ignored": false, "role": {"type": "role", "value": "button"}, "parentId": "2", "name": {"type": "computedString", "value": "Continue"}, "properties": [{"name": "disabled", "value": {"type": "boolean", "value": true}}], "childIds": ["21"], "backendDOMNodeId": 120},
        {"nodeId": "21", "ignored": false, "role": {"type": "internalRole", "value": "StaticText"}, "parentId": "20", "name": {"type": "computedString", "value": "Continue"}, "backendDOMNodeId": 121},
        {"nodeId": "22", "ignored": true, "ignoredReasons": [{"name": "ariaHiddenElement", "value": {"type": "boolean", "value": true}}], "role": {"type": "role", "value": "generic"}, "parentId": "2", "childIds": ["23"], "backendDOMNodeId": 122},
        {"nodeId": "23", "ignored": true, "role": {"type": "role", "value": "button"}, "parentId": "22", "name": {"type": "computedString", "value": "Hidden"}, "backendDOMNodeId": 123},
        {"nodeId": "25", "ignored": false, "role": {"type": "role", "value": "list"}, "parentId": "2", "childIds": ["26"], "backendDOMNodeId": 125},
        {"nodeId": "26", "ignored": false, "role": {"type": "role", "value": "listitem"}, "parentId": "25", "properties": [{"name": "level", "value": {"type": "integer", "value": 1}}], "childIds": ["27", "29"], "backendDOMNodeId": 126},
        {"nodeId": "29", "ignored": false, "role": {"type": "internalRole", "value": "ListMarker"}, "parentId": "26", "name": {"type": "computedString", "value": "• "}, "backendDOMNodeId": 129},
        {"nodeId": "27", "ignored": false, "role": {"type": "internalRole", "value": "StaticText"}, "parentId": "26", "name": {"type": "computedString", "value": "Terms"}, "backendDOMNodeId": 127}
    ]}"#;

    fn sign_in() -> Vec<AxNode> {
        tree(SIGN_IN)
    }

    fn tree(json: &str) -> Vec<AxNode> {
        serde_json::from_str::<GetFullAxTreeReturns>(json)
            .expect("the tree reads as the protocol's own type")
            .nodes
    }

    /// The lines of a page with no frames.
    fn lines(nodes: &[AxNode], interactive: bool) -> Vec<Line> {
        let page = Tree {
            frame: "MAIN".to_owned(),
            owner: None,
            nodes: nodes.to_vec(),
        };
        super::lines(&[page], &HashMap::new(), interactive)
    }

    /// A shop with a checkout frame from its own site, which has a frame of
    /// its own, and an ad from another.
    fn shop() -> Vec<Tree> {
        let page = tree(
            r#"{"nodes": [
            {"nodeId": "1", "ignored": false, "role": {"type": "internalRole", "value": "RootWebArea"}, "name": {"type": "computedString", "value": "Shop"}, "childIds": ["2", "3", "4"], "backendDOMNodeId": 1},
            {"nodeId": "2", "ignored": false, "role": {"type": "role", "value": "heading"}, "parentId": "1", "name": {"type": "computedString", "value": "Shop"}, "backendDOMNodeId": 2},
            {"nodeId": "3", "ignored": false, "role": {"type": "internalRole", "value": "Iframe"}, "parentId": "1", "name": {"type": "computedString", "value": "Checkout"}, "backendDOMNodeId": 50},
            {"nodeId": "4", "ignored": false, "role": {"type": "internalRole", "value": "Iframe"}, "parentId": "1", "name": {"type": "computedString", "value": "Ad"}, "backendDOMNodeId": 60}
        ]}"#,
        );
        // Node ids start again in each frame's tree.
        let checkout = tree(
            r#"{"nodes": [
            {"nodeId": "1", "ignored": false, "role": {"type": "internalRole", "value": "RootWebArea"}, "name": {"type": "computedString", "value": "Checkout"}, "childIds": ["2", "3"], "backendDOMNodeId": 70},
            {"nodeId": "2", "ignored": false, "role": {"type": "role", "value": "textbox"}, "parentId": "1", "name": {"type": "computedString", "value": "Card"}, "backendDOMNodeId": 2},
            {"nodeId": "3", "ignored": false, "role": {"type": "internalRole", "value": "Iframe"}, "parentId": "1", "name": {"type": "computedString", "value": "Terms"}, "backendDOMNodeId": 80}
        ]}"#,
        );
        let terms = tree(
            r#"{"nodes": [
            {"nodeId": "1", "ignored": false, "role": {"type": "internalRole", "value": "RootWebArea"}, "childIds": ["2"], "backendDOMNodeId": 90},
            {"nodeId": "2", "ignored": false, "role": {"type": "role", "value": "button"}, "parentId": "1", "name": {"type": "computedString", "value": "Pay"}, "backendDOMNodeId": 91}
        ]}"#,
        );
        let frame = |frame: &str, owner, nodes| Tree {
            frame: frame.to_owned(),
            owner,
            nodes,
        };
        vec![
            frame("MAIN", None, page),
            frame("TERMS", Some(80), terms),
            frame("CHECKOUT", Some(50), checkout),
        ]
    }

    /// Lines as the CLI would print them, near enough to read.
    fn shown(lines: &[Line]) -> Vec<String> {
        lines
            .iter()
            .map(|line| {
                let mut text = format!(
                    "{}{} {:?}",
                    "  ".repeat(line.depth.into()),
                    line.role,
                    line.name
                );
                if let Some(value) = &line.value {
                    text.push_str(&format!(" value={value:?}"));
                }
                for state in &line.states {
                    text.push(' ');
                    text.push_str(state);
                }
                text
            })
            .collect()
    }

    #[test]
    fn a_snapshot_shows_what_a_reader_would_and_leaves_out_the_rest() {
        let lines = lines(&sign_in(), false);
        assert_eq!(
            shown(&lines),
            [
                r#"heading "Sign in" level=1"#,
                r#"textbox "Email" value="brad@example.com" focused required"#,
                r#"checkbox "Remember me" checked"#,
                r#"paragraph """#,
                r#"  text "New here?""#,
                r#"  link "Create an account""#,
                r#"button "Continue" disabled"#,
                r#"list """#,
                r#"  listitem "Terms""#,
            ]
        );
        // Each line names the DOM node it came from; a folded line keeps
        // its own.
        let nodes: Vec<Option<i64>> = lines.iter().map(|line| line.node).collect();
        assert_eq!(nodes, [3, 105, 111, 15, 116, 117, 120, 125, 126].map(Some));
    }

    #[test]
    fn an_interactive_snapshot_keeps_only_what_can_be_acted_on() {
        assert_eq!(
            shown(&lines(&sign_in(), true)),
            [
                r#"textbox "Email" value="brad@example.com" focused required"#,
                r#"checkbox "Remember me" checked"#,
                r#"link "Create an account""#,
                r#"button "Continue" disabled"#,
            ]
        );
    }

    #[test]
    fn a_tree_that_loops_or_says_nothing_still_ends() {
        let looped: Vec<AxNode> = serde_json::from_str::<GetFullAxTreeReturns>(
            r#"{"nodes": [
                {"nodeId": "1", "ignored": false, "role": {"type": "role", "value": "list"},
                 "childIds": ["2"], "backendDOMNodeId": 1},
                {"nodeId": "2", "ignored": false, "role": {"type": "role", "value": "listitem"},
                 "parentId": "1", "childIds": ["1", "9"], "backendDOMNodeId": 2}
            ]}"#,
        )
        .unwrap()
        .nodes;
        assert_eq!(
            shown(&lines(&looped, false)),
            [r#"list """#, r#"  listitem """#]
        );
        assert!(lines(&[], false).is_empty());
    }

    #[test]
    fn a_frame_from_the_pages_site_shows_under_its_iframe_and_another_sites_says_so() {
        let trees = shop();
        let away = HashMap::from([(60, "ads.example.net".to_owned())]);
        let all = super::lines(&trees, &away, false);
        assert_eq!(
            shown(&all),
            [
                r#"heading "Shop""#,
                r#"Iframe "Checkout""#,
                r#"  textbox "Card""#,
                r#"  Iframe "Terms""#,
                r#"    button "Pay""#,
                r#"Iframe "Ad" cross-site=ads.example.net"#,
            ]
        );
        // Each line knows its frame: backend id 2 is a heading in the page
        // and a field in the checkout.
        let frames: Vec<(&str, Option<i64>)> = all
            .iter()
            .map(|line| (trees[line.frame].frame.as_str(), line.node))
            .collect();
        assert_eq!(
            frames,
            [
                ("MAIN", Some(2)),
                ("MAIN", Some(50)),
                ("CHECKOUT", Some(2)),
                ("CHECKOUT", Some(80)),
                ("TERMS", Some(91)),
                ("MAIN", Some(60)),
            ]
        );
        // Only what can be acted on: the frames' fields and buttons in place
        // of their frames, and another site's frame, which a click reaches.
        assert_eq!(
            shown(&super::lines(&trees, &away, true)),
            [
                r#"textbox "Card""#,
                r#"button "Pay""#,
                r#"Iframe "Ad" cross-site=ads.example.net"#,
            ]
        );
    }

    #[test]
    fn a_frame_tree_lists_every_frame_under_the_page_depth_first() {
        let tree: FrameTree = serde_json::from_value(serde_json::json!({
            "frame": {"id": "MAIN", "loaderId": "L1", "url": "http://localhost:3000/",
                      "domainAndRegistry": "", "securityOrigin": "http://localhost:3000",
                      "mimeType": "text/html", "secureContextType": "InsecureScheme",
                      "crossOriginIsolatedContextType": "NotIsolated", "gatedAPIFeatures": []},
            "childFrames": [
                {"frame": {"id": "A", "parentId": "MAIN", "loaderId": "L2", "url": "about:blank",
                           "domainAndRegistry": "", "securityOrigin": "://",
                           "mimeType": "text/html", "secureContextType": "InsecureScheme",
                           "crossOriginIsolatedContextType": "NotIsolated", "gatedAPIFeatures": []},
                 "childFrames": [
                    {"frame": {"id": "A1", "parentId": "A", "loaderId": "L3", "url": "about:blank",
                               "domainAndRegistry": "", "securityOrigin": "://",
                               "mimeType": "text/html", "secureContextType": "InsecureScheme",
                               "crossOriginIsolatedContextType": "NotIsolated", "gatedAPIFeatures": []}}
                 ]},
                {"frame": {"id": "B", "parentId": "MAIN", "loaderId": "L4", "url": "about:blank",
                           "domainAndRegistry": "", "securityOrigin": "://",
                           "mimeType": "text/html", "secureContextType": "InsecureScheme",
                           "crossOriginIsolatedContextType": "NotIsolated", "gatedAPIFeatures": []}}
            ]
        }))
        .expect("a frame tree as the protocol gives it");
        assert_eq!(frames_in(tree), ["A", "A1", "B"]);
        let attributes = ["id", "pay", "src", "https://js.stripe.com/v3/"].map(str::to_owned);
        assert_eq!(
            attribute(Some(&attributes), "src"),
            "https://js.stripe.com/v3/"
        );
        assert_eq!(attribute(Some(&attributes), "title"), "");
        assert_eq!(attribute(None, "src"), "");
    }

    #[test]
    fn long_names_are_cut_and_white_space_squashed() {
        assert_eq!(squash("  Sign \n in  "), "Sign in");
        let long = cut("é".repeat(NAME_MAX + 5), NAME_MAX);
        assert_eq!(long.chars().count(), NAME_MAX);
        assert!(long.ends_with('…'));
        assert_eq!(cut("short".to_owned(), NAME_MAX), "short");
    }
}
