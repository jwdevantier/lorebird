//! Compose-mail logic: pre-filled replies, RFC 2822 serialisation.
//!
//! `ComposeMail` mirrors the Lua `mail` table from the spec and carries
//! all data needed for the compose window.  `ParentMail` carries the
//! read-only parent information passed to the `on_reply` hook.
//!
//! See `specs/app_config_and_email_compose.md` for the full design.

use std::collections::HashMap;

// ── Data types ────────────────────────────────────────────────────────

/// A mail being composed (reply or new).
///
/// Fields mirror the Lua `mail` table from the spec.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ComposeMail {
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub cc: String,
    #[serde(default)]
    pub bcc: String,
    pub subject: String,
    #[serde(default)]
    pub in_reply_to: Option<String>,
    #[serde(default)]
    pub references: Option<String>,
    pub body_text: String,
    /// Arbitrary extra headers (e.g. X-Mailer, Reply-To).
    #[serde(default)]
    pub headers: HashMap<String, String>,
}

/// Parent message data passed to the `on_reply` hook.
///
/// Mirrors the `parent` table from the spec.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ParentMail {
    pub message_id: Option<String>,
    pub from: String,
    #[serde(default)]
    pub to: String,
    #[serde(default)]
    pub cc: String,
    pub subject: String,
    pub date: String,
    pub references: String,
    pub in_reply_to: Option<String>,
    pub body_text: String,
}

// ── Reply construction ────────────────────────────────────────────────

impl ComposeMail {
    /// Build a pre-filled reply from a parent message and profile.
    ///
    /// Follows standard reply conventions:
    /// - `to` = parent's `from`
    /// - `cc` = parent's `cc`
    /// - `subject` = "Re: " prefix (no double Re:)
    /// - `in_reply_to` = parent's `message_id`
    /// - `references` = parent's references + " " + parent's message_id
    /// - `from` = "Profile Name <profile@email>"
    /// - `body_text` = quoted parent body with attribution line
    /// Build a pre-filled reply from a parent message and profile.
    ///
    /// Reply-All (mailing-list style):
    /// - `to`  = parent's `to` addresses + self (added if not already present)
    /// - `cc`  = parent's `cc` addresses minus self
    /// - `subject` = "Re: " prefix (no double Re:)
    /// - `in_reply_to` = parent's `message_id`
    /// - `references` = parent's references + " " + parent's `message_id`
    /// - `from` = "Profile Name <profile@email>"
    /// - `body_text` = quoted parent body with attribution line
    pub fn new_reply(parent: &ParentMail, profile_name: &str, profile_email: &str) -> Self {
        let from = format!("{} <{}>", profile_name, profile_email);
        let to = add_self_to_addrs(&parent.to, profile_name, profile_email);
        let cc = remove_self_from_addrs(&parent.cc, profile_email);

        // Subject: prepend "Re: " unless it already starts with it (case-insensitive).
        let subject = if parent.subject.to_lowercase().starts_with("re:") {
            parent.subject.clone()
        } else {
            format!("Re: {}", parent.subject)
        };

        let in_reply_to = parent.message_id.clone();

        // References chain: parent's references + parent's message_id.
        let mut refs_parts = Vec::new();
        if !parent.references.is_empty() {
            refs_parts.push(parent.references.clone());
        }
        if let Some(ref mid) = parent.message_id {
            refs_parts.push(mid.clone());
        }
        let references = if refs_parts.is_empty() {
            None
        } else {
            Some(refs_parts.join(" "))
        };

        let body_text = format_reply_body(&parent.body_text, &parent.date, &parent.from);

        ComposeMail {
            from,
            to,
            cc,
            bcc: String::new(),
            subject,
            in_reply_to,
            references,
            body_text,
            headers: HashMap::new(),
        }
    }

    /// Serialize this mail to RFC 2822 format.
    ///
    /// Produces a string suitable for piping to sendmail `-t` or
    /// saving as a draft.  Headers are written in a deterministic
    /// order followed by arbitrary user headers (sorted), then a
    /// blank line, then the body.
    pub fn to_rfc2822(&self) -> String {
        let mut out = String::new();

        out.push_str(&format!("From: {}\n", self.from));
        out.push_str(&format!("To: {}\n", self.to));
        if !self.cc.is_empty() {
            out.push_str(&format!("Cc: {}\n", self.cc));
        }
        if !self.bcc.is_empty() {
            out.push_str(&format!("Bcc: {}\n", self.bcc));
        }
        out.push_str(&format!("Subject: {}\n", self.subject));
        if let Some(ref irt) = self.in_reply_to {
            out.push_str(&format!("In-Reply-To: {}\n", irt));
        }
        if let Some(ref refs) = self.references {
            out.push_str(&format!("References: {}\n", refs));
        }

        // Arbitrary headers in sorted order for determinism.
        let mut headers: Vec<_> = self.headers.iter().collect();
        headers.sort_by_key(|(k, _)| *k);
        for (key, value) in headers {
            out.push_str(&format!("{}: {}\n", key, value));
        }

        // Blank line separating headers from body
        out.push('\n');
        out.push_str(&self.body_text);
        out.push('\n');

        out
    }
}

// ── Helpers ───────────────────────────────────────────────────────────

/// Format the reply body with attribution and quoted text.
///
/// Produces:
/// ```text
///
/// On <date>, <from> wrote:
/// > line 1
/// > line 2
/// ```
fn format_reply_body(body: &str, date: &str, from: &str) -> String {
    let mut out = String::new();
    out.push_str("\n\n");
    out.push_str(&format!("On {}, {} wrote:\n", date, from));
    for line in body.lines() {
        out.push('>');
        if !line.is_empty() {
            out.push(' ');
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}

/// Add our own address to a comma-separated RFC 2822 address list
/// if not already present.
///
/// Addresses are `"Name <email>"` or bare `email`, separated
/// by `", "`.  We detect ourselves by email substring matching.
fn add_self_to_addrs(addrs: &str, name: &str, email: &str) -> String {
    if addr_list_contains(addrs, email) {
        addrs.to_string()
    } else if addrs.is_empty() {
        format!("{} <{}>", name, email)
    } else {
        format!("{}, {} <{}>", addrs, name, email)
    }
}

/// Remove our own address from a comma-separated RFC 2822 address list.
///
/// Splits on `", "`, removes any entry containing our email (matching
/// both `alice@example.com` and `Alice <alice@example.com>`), and
/// re-joins with `", "`.
fn remove_self_from_addrs(addrs: &str, email: &str) -> String {
    if addrs.is_empty() || !addr_list_contains(addrs, email) {
        return addrs.to_string();
    }
    let filtered: Vec<&str> = addrs
        .split(", ")
        .filter(|entry| !entry.contains(email))
        .collect();
    filtered.join(", ")
}

/// Check whether a comma-separated RFC 2822 address list contains the
/// given bare email.
///
/// Matches both `alice@example.com` and `Alice <alice@example.com>`
/// by testing substring containment on each comma-separated entry.
fn addr_list_contains(addrs: &str, email: &str) -> bool {
    addrs
        .split(", ")
        .any(|entry| entry.contains(email))
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_parent() -> ParentMail {
        ParentMail {
            message_id: Some("<abc@def>".to_string()),
            from: "Alice <alice@example.com>".to_string(),
            to: "list@example.com".to_string(),
            cc: "bob@example.com, carol@example.com".to_string(),
            subject: "[PATCH v2] Fix memory leak".to_string(),
            date: "2024-01-15T10:30:00+00:00".to_string(),
            references: "<parent@def> <other@def>".to_string(),
            in_reply_to: Some("<parent@def>".to_string()),
            body_text: "This patch fixes...\n".to_string(),
        }
    }

    #[test]
    fn new_reply_basic() {
        let parent = make_parent();
        let mail = ComposeMail::new_reply(&parent, "Riccardo", "riccardo@defmacro.it");

        assert_eq!(mail.from, "Riccardo <riccardo@defmacro.it>");
        // To = parent.to + self (self not in parent.to)
        assert_eq!(mail.to, "list@example.com, Riccardo <riccardo@defmacro.it>");
        // Cc copied from parent, self not in parent.cc so unchanged
        assert_eq!(mail.cc, "bob@example.com, carol@example.com");
        assert_eq!(mail.bcc, "");
        assert_eq!(mail.subject, "Re: [PATCH v2] Fix memory leak");
        assert_eq!(mail.in_reply_to, Some("<abc@def>".to_string()));
        assert_eq!(
            mail.references,
            Some("<parent@def> <other@def> <abc@def>".to_string())
        );
        assert!(mail.body_text.contains("On 2024-01-15T10:30:00+00:00, Alice <alice@example.com> wrote:"));
        assert!(mail.body_text.contains("> This patch fixes..."));
    }

    #[test]
    fn new_reply_self_already_in_to() {
        let parent = ParentMail {
            to: "list@example.com, riccardo@defmacro.it".to_string(),
            ..make_parent()
        };
        let mail = ComposeMail::new_reply(&parent, "Riccardo", "riccardo@defmacro.it");
        // Self already present — To unchanged
        assert_eq!(mail.to, "list@example.com, riccardo@defmacro.it");
    }

    #[test]
    fn new_reply_removes_self_from_cc() {
        let parent = ParentMail {
            cc: "riccardo@defmacro.it, bob@example.com".to_string(),
            ..make_parent()
        };
        let mail = ComposeMail::new_reply(&parent, "Riccardo", "riccardo@defmacro.it");
        // Cc has self removed
        assert_eq!(mail.cc, "bob@example.com");
    }

    #[test]
    fn new_reply_removes_self_from_cc_angle_form() {
        let parent = ParentMail {
            cc: "Riccardo Maffulli <riccardo@defmacro.it>, bob@example.com".to_string(),
            ..make_parent()
        };
        let mail = ComposeMail::new_reply(&parent, "Riccardo", "riccardo@defmacro.it");
        // The whole “Riccardo Maffulli <riccardo@defmacro.it>” entry is removed
        assert_eq!(mail.cc, "bob@example.com");
    }

    #[test]
    fn new_reply_empty_to_adds_self() {
        let parent = ParentMail {
            to: String::new(),
            ..make_parent()
        };
        let mail = ComposeMail::new_reply(&parent, "Riccardo", "riccardo@defmacro.it");
        assert_eq!(mail.to, "Riccardo <riccardo@defmacro.it>");
    }

    #[test]
    fn new_reply_empty_cc_stays_empty() {
        let parent = ParentMail {
            cc: String::new(),
            ..make_parent()
        };
        let mail = ComposeMail::new_reply(&parent, "Riccardo", "riccardo@defmacro.it");
        assert_eq!(mail.cc, "");
    }

    #[test]
    fn no_double_re_prefix() {
        let parent = ParentMail {
            subject: "Re: Already a reply".to_string(),
            ..make_parent()
        };
        let mail = ComposeMail::new_reply(&parent, "Bob", "bob@example.com");
        assert_eq!(mail.subject, "Re: Already a reply");
    }

    #[test]
    fn case_insensitive_re_detection() {
        let parent = ParentMail {
            subject: "RE: Mixed case".to_string(),
            ..make_parent()
        };
        let mail = ComposeMail::new_reply(&parent, "Bob", "bob@example.com");
        assert_eq!(mail.subject, "RE: Mixed case");
    }

    #[test]
    fn new_reply_no_message_id() {
        let mut parent = make_parent();
        parent.message_id = None;
        parent.references = String::new();
        let mail = ComposeMail::new_reply(&parent, "Bob", "bob@example.com");
        assert_eq!(mail.in_reply_to, None);
        assert_eq!(mail.references, None);
    }

    #[test]
    fn format_reply_body_wraps_lines() {
        let body = "Line one\nLine two\n";
        let result = format_reply_body(body, "2024-01-15", "Alice");
        assert!(result.contains("On 2024-01-15, Alice wrote:"));
        assert!(result.contains("> Line one"));
        assert!(result.contains("> Line two"));
    }

    #[test]
    fn format_reply_body_empty_line() {
        let body = "Hello\n\nWorld\n";
        let result = format_reply_body(body, "2024-01-15", "Bob");
        assert!(result.contains("> Hello"));
        assert!(result.contains(">\n")); // empty line just has >
        assert!(result.contains("> World"));
    }

    #[test]
    fn to_rfc2822_basic() {
        let mut mail = ComposeMail {
            from: "Bob <bob@example.com>".to_string(),
            to: "Alice <alice@example.com>".to_string(),
            cc: "list@example.com".to_string(),
            bcc: String::new(),
            subject: "Re: Test".to_string(),
            in_reply_to: Some("<parent@example.com>".to_string()),
            references: Some("<grandparent@example.com> <parent@example.com>".to_string()),
            body_text: "Hello\n".to_string(),
            headers: HashMap::new(),
        };
        mail.headers.insert("X-Mailer".to_string(), "loreread".to_string());

        let rfc = mail.to_rfc2822();
        assert!(rfc.starts_with("From: Bob <bob@example.com>\n"));
        assert!(rfc.contains("To: Alice <alice@example.com>\n"));
        assert!(rfc.contains("Cc: list@example.com\n"));
        assert!(!rfc.contains("Bcc:")); // empty Bcc omitted
        assert!(rfc.contains("Subject: Re: Test\n"));
        assert!(rfc.contains("In-Reply-To: <parent@example.com>\n"));
        assert!(rfc.contains("References: <grandparent@example.com> <parent@example.com>\n"));
        assert!(rfc.contains("X-Mailer: loreread\n"));
        // Blank line + body
        assert!(rfc.contains("\nHello\n"));
    }

    #[test]
    fn to_rfc2822_minimal() {
        let mail = ComposeMail {
            from: "A <a@b>".to_string(),
            to: "C <c@d>".to_string(),
            cc: String::new(),
            bcc: String::new(),
            subject: "Hello".to_string(),
            in_reply_to: None,
            references: None,
            body_text: "Body\n".to_string(),
            headers: HashMap::new(),
        };
        let rfc = mail.to_rfc2822();
        assert!(!rfc.contains("In-Reply-To:"));
        assert!(!rfc.contains("References:"));
        assert!(!rfc.contains("Cc:"));
    }
}