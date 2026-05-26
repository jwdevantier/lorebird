//! JWZ email threading algorithm.
//!
//! Based on <https://www.jwz.org/doc/threading.html>.
//!
//! The algorithm groups messages into trees based on:
//! - `Message-ID` (unique per message)
//! - `References` + `In-Reply-To` headers (parent links)
//! - `Subject` fallback (bare subject, stripped of "Re:", "Fwd:", etc.)

use std::cmp::Ordering;
use std::collections::HashMap;

/// Minimal data the threading algorithm needs from each message.
///
/// Implement this trait for whatever message type your application uses
/// (e.g. a parsed mail, a database row, etc.).
pub trait Message {
    /// The `Message-ID` header value, without angle brackets.
    /// Returns `None` if the message has no ID (malformed).
    fn message_id(&self) -> Option<&str>;

    /// All references to parent messages, in order.
    ///
    /// Typically this is the `References` header followed by the
    /// `In-Reply-To` header (if not already present in `References`).
    /// Each entry should be a message ID without angle brackets.
    fn references(&self) -> &[String];

    /// The `Subject` header value. Used as a fallback when no reference
    /// links exist: messages sharing the same "bare" subject (after
    /// stripping "Re:", "Fwd:", etc.) are grouped together.
    fn subject(&self) -> Option<&str>;
}

#[derive(Debug)]
struct Container<T> {
    /// None for ghost/placeholder containers
    message: Option<T>,

    parent: Option<usize>,
    children: Vec<usize>,
}

impl<T> Container<T> {
    fn new(message: Option<T>) -> Self {
        Self {
            message,
            parent: None,
            children: vec![],
        }
    }
}

// ── Thread tree ──────────────────────────────────────────────────────

/// Internal representation of a Thread
///
/// During processing, there will be some time during which
/// we may have containers without a message of their own
/// but which form the root of a thread.
/// Subsequent cleanup will handle this
#[derive(Debug)]
pub struct ThreadNode<T: Message> {
    pub message: Option<T>,
    pub children: Vec<ThreadNode<T>>,
}

/// A node in a thread tree.
#[derive(Debug)]
pub struct Thread<T: Message> {
    /// The message at this node.
    pub message: T,
    /// Child messages (replies).
    pub children: Vec<Thread<T>>,
}

// ── Algorithm ────────────────────────────────────────────────────────

fn is_ancestor<T>(ancestor_idx: usize, descendant_idx: usize, lst: &[Container<T>]) -> bool {
    let mut cur = lst[descendant_idx].parent;
    while let Some(p) = cur {
        if p == ancestor_idx {
            return true;
        }
        cur = lst[p].parent;
    }
    false
}

fn build_thread_subtree<T: Message>(
    root_idx: usize,
    cs: &mut Vec<Container<T>>,
) -> Vec<ThreadNode<T>> {
    let children: Vec<ThreadNode<T>> = cs[root_idx]
        .children
        .clone()
        .into_iter()
        .flat_map(|child_idx| build_thread_subtree(child_idx, cs))
        .collect();

    let msg = cs[root_idx].message.take();
    let is_root_ghost = cs[root_idx].parent.is_none();

    // * container w/o children or msg -> NUKE
    // * container w/o msg, WITH children ->
    //    * remove, promote children
    //    * UNLESS promoting children to root level
    //        * IF there's just one child to promote this way, do it anyway
    // TODO: to remove a container, we need to:
    //       - remove the container from parent.children
    //       - change the .parent ref in each of `children` to the parent of the removed container
    match (msg, children.len(), is_root_ghost) {
        (Some(msg), _, _) => vec![ThreadNode {
            message: Some(msg),
            children,
        }],
        (None, 0, _) => vec![],       // ghost node, remove
        (None, 1, _) => children,     // empty container, single child, always promote
        (None, _, false) => children, // promote
        (None, _, true) => vec![ThreadNode {
            // top-level ghost, multiple children, retain
            message: None,
            children,
        }],
    }
}

fn _bare_subject(raw: &str) -> (String, usize) {
    let mut s = raw.trim().to_string();
    let mut depth = 0;
    loop {
        let trimmed = s.trim_start();
        let lower = trimmed.to_lowercase();
        let rest = lower.strip_prefix("re:").or_else(|| {
            let x = lower.strip_prefix("re")?.trim_start().strip_prefix('[')?;
            let close = x.find(']')?;
            x[close..].strip_prefix("]:")
        });
        match rest {
            Some(r) => {
                depth += 1;
                s = trimmed[trimmed.len() - r.len()..].trim().to_string();
            }
            None => break,
        }
    }
    (s, depth)
}

/// Strip leading "re:", "re[4]:" (case-insensitive)
/// May return empty string if nothing else remains in the subject line
fn bare_subject(raw: &str) -> String {
    let (s, _) = _bare_subject(raw);
    return s;
}

/// Find first occurrence of a message in subtree and extract its subject
fn subtree_subject<T: Message>(node: &ThreadNode<T>) -> Option<String> {
    if let Some(ref msg) = node.message {
        msg.subject().map(|s| s.to_string())
    } else {
        node.children
            .iter()
            .find_map(|child| subtree_subject(child))
    }
}

fn re_depth(subject: &str) -> usize {
    let (_, depth) = _bare_subject(subject);
    depth
}

fn subject_re_order<T: Message>(a: &ThreadNode<T>, b: &ThreadNode<T>) -> Ordering {
    match (a.message.as_ref(), b.message.as_ref()) {
        (None, Some(_)) => Ordering::Less,
        (Some(_), None) => Ordering::Greater,
        (None, None) => Ordering::Equal,
        (Some(amsg), Some(bmsg)) => {
            let a_d = re_depth(amsg.subject().unwrap_or(""));
            let b_d = re_depth(bmsg.subject().unwrap_or(""));
            a_d.cmp(&b_d)
        }
    }
}

fn group_by_subject<T: Message>(threads: Vec<ThreadNode<T>>) -> Vec<ThreadNode<T>> {
    let mut subject_table: HashMap<String, ThreadNode<T>> = HashMap::new();
    let mut orphans: Vec<ThreadNode<T>> = Vec::new(); // no subject -> pass-through

    for node in threads {
        let bare = match subtree_subject(&node) {
            Some(s) => bare_subject(&s),
            None => {
                orphans.push(node);
                continue;
            }
        };
        if bare.is_empty() {
            orphans.push(node);
            continue;
        }

        match subject_table.remove(&bare) {
            None => {
                subject_table.insert(bare, node);
            }
            Some(existing) => {
                let (winner, loser) = match subject_re_order(&node, &existing) {
                    Ordering::Less => (node, existing),
                    _ => (existing, node),
                };
                subject_table.insert(bare, merge_threads(winner, loser));
            }
        }
    }

    subject_table.into_values().chain(orphans).collect()
}

fn merge_threads<T: Message>(winner: ThreadNode<T>, loser: ThreadNode<T>) -> ThreadNode<T> {
    match (winner.message.is_none(), loser.message.is_none()) {
        // Both empty → merge children into winner
        (true, true) => {
            let mut winner = winner;
            winner.children.extend(loser.children);
            winner
        }
        // Winner is ghost, loser is real → real becomes child of ghost
        (true, false) => {
            let mut winner = winner;
            winner.children.push(loser);
            winner
        }
        // Both real → siblings under a synthetic empty root
        (false, false) => ThreadNode {
            message: None,
            children: vec![winner, loser],
        },
        // Loser is ghost, winner is real — shouldn't happen, but handle gracefully
        (false, true) => {
            let mut loser = loser;
            loser.children.push(winner);
            loser
        }
    }
}

fn finalize<T: Message>(nodes: Vec<ThreadNode<T>>) -> Vec<Thread<T>> {
    nodes
        .into_iter()
        .filter_map(|node| {
            node.message.map(|msg| Thread {
                message: msg,
                children: finalize(node.children),
            })
        })
        .collect()
}

/// Run the JWZ threading algorithm on an unordered collection of messages.
///
/// Returns the root nodes of the thread tree — messages that have no
/// parent in the set. Messages sharing the same bare subject without
/// explicit reference links are grouped under a synthetic root.
pub fn thread_messages<T: Message>(messages: impl IntoIterator<Item = T>) -> Vec<Thread<T>> {
    let mut cs: Vec<Container<T>> = Vec::new();
    let mut seen: HashMap<String, usize> = HashMap::new();

    for msg in messages {
        // For each message, ensure Container exists
        let c_idx = if let Some(id) = msg.message_id() {
            *seen.entry(id.to_string()).or_insert_with(|| {
                let idx = cs.len();
                cs.push(Container::new(None));
                idx
            })
        } else {
            // Message ID missing, assign synthetic one.
            // At later stage, will attempt to build threads from Subject line
            let idx = cs.len();
            cs.push(Container::new(None));
            idx
        };

        // (Create) and link all containers for messages identified in 'References' header
        let mut prev: Option<usize> = None;
        for ref_msgid in msg.references() {
            let ref_idx = *seen.entry(ref_msgid.clone()).or_insert_with(|| {
                let idx = cs.len();
                cs.push(Container::new(None));
                idx
            });

            // should link 'prev' (if set) as parent of current
            //   EXCEPT - we can't add a link if it introduces a loop
            //   Before A->B;
            //   * search down B to see if A is reachable
            //   * search Down A to see if B is reachable
            //
            //   If EITHER is reachable from the other, don't add a link
            if let Some(p) = prev
                && cs[p].parent.is_none()
                && !is_ancestor(p, ref_idx, &cs)
                && !is_ancestor(ref_idx, p, &cs)
            {
                cs[p].parent = Some(ref_idx);
                cs[ref_idx].children.push(p);
            }

            prev = Some(ref_idx)
        }
        //FINALLY; add LAST element in References (** augmented with In-Reply-To)
        //         as the parent of this message
        if let Some(actual_parent_idx) = prev {
            if let Some(parent_idx) = cs[c_idx].parent {
                // already has a parent - ensure we remove this
                // container as a child before changing its parent value
                let cp = &mut cs[parent_idx];
                cp.children.retain(|&idx| idx != c_idx)
            }
            let c = &mut cs[c_idx];
            c.parent = Some(actual_parent_idx);
            let cp = &mut cs[actual_parent_idx];
            cp.children.push(c_idx);
        }

        cs[c_idx].message = Some(msg);
    }

    // Step 2 - find the root set
    let cs_roots: Vec<usize> = cs
        .iter()
        .enumerate()
        .filter_map(|(ndx, val)| match val.parent {
            None => Some(ndx),
            Some(_) => None,
        })
        .collect();

    // Step 3 - can discard `seen`
    drop(seen);

    // Step 4 - Prune empty containers
    //
    //  Removing a container:
    //  remove parent.children ref
    //  re-parent all its children to ITS parent
    let threads: Vec<ThreadNode<T>> = cs_roots
        .into_iter()
        .flat_map(|idx| build_thread_subtree(idx, &mut cs))
        .collect();

    // old hierarchy in `cs` is stale - `build_thread_subtree`
    // calls built the new hierarchy.
    drop(cs);

    // Step 5 - Group root set by subject
    let threads = group_by_subject(threads);

    // Step 6 - Convert ThreadNode → Thread (all messages should be Some by now)
    finalize(threads)
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Simple test message for unit tests.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct TestMessage {
        id: String,
        refs: Vec<String>,
        subject: String,
    }

    impl Message for TestMessage {
        fn message_id(&self) -> Option<&str> {
            Some(&self.id)
        }

        fn references(&self) -> &[String] {
            &self.refs
        }

        fn subject(&self) -> Option<&str> {
            Some(&self.subject)
        }
    }

    /// Helper: collect message IDs in DFS order for easy assertions.
    fn collect_ids<T: Message>(threads: &[Thread<T>]) -> Vec<Vec<String>> {
        fn walk<T: Message>(t: &Thread<T>, acc: &mut Vec<String>) {
            acc.push(t.message.message_id().unwrap().to_string());
            for child in &t.children {
                walk(child, acc);
            }
        }
        threads
            .iter()
            .map(|t| {
                let mut ids = Vec::new();
                walk(t, &mut ids);
                ids
            })
            .collect()
    }

    // ── Bare Subject Tests ───────────────────────────────────────
    #[test]
    fn subj_no_prefix() {
        assert_eq!(bare_subject("Hello world"), "Hello world");
    }

    #[test]
    fn subj_simple_re() {
        assert_eq!(bare_subject("Re: Hello"), "Hello");
    }

    #[test]
    fn subj_uppercase_re() {
        assert_eq!(bare_subject("RE: Hello"), "Hello");
    }

    #[test]
    fn subj_mixed_case_re() {
        assert_eq!(bare_subject("rE: Hello"), "Hello");
    }

    #[test]
    fn subj_numbered_re() {
        assert_eq!(bare_subject("Re[5]: Hello"), "Hello");
    }

    #[test]
    fn subj_chained_re_prefixes() {
        assert_eq!(bare_subject("Re: Re[4]: Re: Hello"), "Hello");
    }

    #[test]
    fn subj_only_prefix_no_content() {
        assert_eq!(bare_subject("Re: "), "");
    }

    #[test]
    fn subj_only_re_with_colon() {
        assert_eq!(bare_subject("Re:"), "");
    }

    #[test]
    fn subj_leading_whitespace() {
        assert_eq!(bare_subject("   Re: Hello"), "Hello");
    }

    #[test]
    fn subj_empty_string() {
        assert_eq!(bare_subject(""), "");
    }

    #[test]
    fn subj_whitespace_only() {
        assert_eq!(bare_subject("   "), "");
    }

    #[test]
    fn subj_not_a_re_prefix() {
        // "Regarding: Hello" starts with "Re" but isn't a Re: prefix
        assert_eq!(bare_subject("Regarding: Hello"), "Regarding: Hello");
    }

    #[test]
    fn subj_re_in_middle_of_word() {
        // "Re" must be at the start (after whitespace) to count
        assert_eq!(bare_subject("Care: Hello"), "Care: Hello");
    }

    /*
    // ── Basic linear thread ───────────────────────────────────────

    #[test]
    fn linear_thread_by_references() {
        let messages = vec![
            TestMessage {
                id: "a".into(),
                refs: vec![],
                subject: "Hello".into(),
            },
            TestMessage {
                id: "b".into(),
                refs: vec!["a".into()],
                subject: "Re: Hello".into(),
            },
            TestMessage {
                id: "c".into(),
                refs: vec!["a".into(), "b".into()],
                subject: "Re: Hello".into(),
            },
        ];

        let threads = thread_messages(messages);

        let ids = collect_ids(&threads);
        // One thread: a → b → c
        assert_eq!(ids.len(), 1);
        assert_eq!(ids[0], vec!["a", "b", "c"]);
    }

    // ── Missing parent (ghost message) ────────────────────────────

    #[test]
    fn ghost_parent() {
        let messages = vec![TestMessage {
            id: "b".into(),
            refs: vec!["a".into()],
            subject: "Re: Hello".into(),
        }];

        let threads = thread_messages(messages);

        // "a" is a ghost — a synthetic empty root should appear?
        // or "b" becomes a root. Decide based on JWZ spec.
        let ids = collect_ids(&threads);
        assert_eq!(ids.len(), 1);
        // TODO: define expected behavior for ghost parent
    }

    // ── Subject-based grouping ────────────────────────────────────

    #[test]
    fn subject_based_grouping() {
        let messages = vec![
            TestMessage {
                id: "1".into(),
                refs: vec![],
                subject: "Meeting notes".into(),
            },
            TestMessage {
                id: "2".into(),
                refs: vec![],
                subject: "Re: Meeting notes".into(),
            },
            TestMessage {
                id: "3".into(),
                refs: vec![],
                subject: "Fwd: Meeting notes".into(),
            },
        ];

        let threads = thread_messages(messages);

        // All three should be under one synthetic root (same bare subject)
        let ids = collect_ids(&threads);
        assert_eq!(ids.len(), 1);
        assert_eq!(ids[0].len(), 3);
    }

    // ── Multiple independent threads ──────────────────────────────

    #[test]
    fn independent_threads() {
        let messages = vec![
            TestMessage {
                id: "x".into(),
                refs: vec![],
                subject: "Alpha".into(),
            },
            TestMessage {
                id: "y".into(),
                refs: vec!["x".into()],
                subject: "Re: Alpha".into(),
            },
            TestMessage {
                id: "p".into(),
                refs: vec![],
                subject: "Beta".into(),
            },
            TestMessage {
                id: "q".into(),
                refs: vec![],
                subject: "Gamma".into(),
            },
        ];

        let threads = thread_messages(messages);

        let ids = collect_ids(&threads);
        // Three threads: [x → y], [p], [q]
        assert_eq!(ids.len(), 3);
        // Order not guaranteed, so check by set
        let mut sorted: Vec<Vec<String>> = ids
            .into_iter()
            .map(|mut v| {
                v.sort();
                v
            })
            .collect();
        sorted.sort_by(|a, b| a[0].cmp(&b[0]));
        assert_eq!(sorted[0], vec!["p"]);
        assert_eq!(sorted[1], vec!["q"]);
        assert_eq!(sorted[2], vec!["x", "y"]);
    }

    // ── Message with no ID ───────────────────────────────────────

    #[test]
    fn message_without_id() {
        // Messages without a Message-ID can't be threaded by reference,
        // but could still be threaded by subject.
        // TODO: define expected behavior
    }
    */
}
