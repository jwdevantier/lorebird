//! Bridge between `mail_parser::Message` and our domain types.
//!
//! Extracts the fields we care about (message ID, references, subject, etc.)
//! from a parsed raw message and provides the `thread::Message` trait impl.

use mail_parser::HeaderValue;

use crate::thread;

/// Extracted message fields, ready for threading or indexing.
#[derive(Debug, Clone)]
pub struct MailMessage {
    pub message_id: Option<String>,
    pub references: Vec<String>,
    pub subject: Option<String>,
    pub from_addr: Option<String>,
    pub to_addr: Option<String>,
    pub cc_addr: Option<String>,
    pub date_rfc3339: Option<String>,
    pub date_ts: i64,
    pub received_ts: i64,
    pub body_text: Option<String>,
}

impl MailMessage {
    /// Parse raw message bytes into a `MailMessage`.
    pub fn from_bytes(raw: &[u8]) -> Option<Self> {
        let msg = mail_parser::MessageParser::default().parse(raw)?;

        let message_id = msg.message_id().map(|s| s.to_string());

        let references = msg
            .references()
            .iter_ids()
            .map(|s| s.to_string())
            .chain(
                msg.in_reply_to()
                    .iter_ids()
                    .map(|s| s.to_string()),
            )
            .collect::<Vec<_>>();

        // Deduplicate while preserving order
        let mut seen = std::collections::HashSet::new();
        let references: Vec<String> = references
            .into_iter()
            .filter(|id| seen.insert(id.clone()))
            .collect();

        let subject = msg.subject().map(|s| s.to_string());

        let from_addr = first_addr(msg.from());
        let to_addr = all_addrs(msg.to());
        let cc_addr = all_addrs(msg.cc());

        let date_rfc3339 = msg.date().map(|d| d.to_rfc3339());
        let date_ts = msg.date().map(|d| d.to_timestamp() as i64).unwrap_or(0);

        let received_ts = msg
            .received()
            .and_then(|r| r.date)
            .map(|d| d.to_timestamp() as i64)
            .unwrap_or(0);

        let body_text = msg.body_text(0).map(|s| s.to_string());

        Some(Self {
            message_id,
            references,
            subject,
            from_addr,
            to_addr,
            cc_addr,
            date_rfc3339,
            date_ts,
            received_ts,
            body_text,
        })
    }
}

/// Extract the first email address from a mail_parser `Address` enum.
fn first_addr(addr: Option<&mail_parser::Address>) -> Option<String> {
    use mail_parser::Address;
    addr.and_then(|a| match a {
        Address::List(addrs) => addrs.first().and_then(|a| a.address.clone().map(|s| s.to_string())),
        Address::Group(groups) => groups.first().and_then(|g| g.addresses.first().and_then(|a| a.address.clone().map(|s| s.to_string()))),
    })
}

/// Collect all email addresses from a mail_parser `Address` enum into a
/// single space-separated string (for FTS5 tokenization).
fn all_addrs(addr: Option<&mail_parser::Address>) -> Option<String> {
    use mail_parser::Address;
    let addrs: Vec<String> = addr
        .map(|a| match a {
            Address::List(list) => list.iter().filter_map(|a| a.address.clone()).map(|s| s.to_string()).collect(),
            Address::Group(groups) => groups.iter().flat_map(|g| g.addresses.iter().filter_map(|a| a.address.clone().map(|s| s.to_string()))).collect(),
        })
        .unwrap_or_default();
    if addrs.is_empty() { None } else { Some(addrs.join(" ")) }
}

/// Extension trait to extract message IDs from a `HeaderValue`.
trait HeaderValueIdIter {
    fn iter_ids(&self) -> Box<dyn Iterator<Item = &str> + '_>;
}

impl HeaderValueIdIter for HeaderValue<'_> {
    fn iter_ids(&self) -> Box<dyn Iterator<Item = &str> + '_> {
        match self {
            HeaderValue::Text(s) => Box::new(s.split_whitespace()),
            HeaderValue::TextList(list) => Box::new(list.iter().map(|s| s.as_ref())),
            _ => Box::new(std::iter::empty()),
        }
    }
}

// ── thread::Message impl ─────────────────────────────────────────────

impl thread::Message for MailMessage {
    fn message_id(&self) -> Option<&str> {
        self.message_id.as_deref()
    }

    fn references(&self) -> &[String] {
        &self.references
    }

    fn subject(&self) -> Option<&str> {
        self.subject.as_deref()
    }

    fn received_ts(&self) -> i64 {
        self.received_ts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal_message() {
        let raw = b"From: a@b.com\r\nSubject: Hello\r\nMessage-ID: <abc@def>\r\n\r\nhi";
        let msg = MailMessage::from_bytes(raw).unwrap();
        assert_eq!(msg.subject.as_deref(), Some("Hello"));
        assert_eq!(msg.message_id.as_deref(), Some("abc@def"));
    }

    #[test]
    fn parse_with_references() {
        let raw = concat!(
            "From: x@y.com\r\n",
            "Subject: Re: topic\r\n",
            "Message-ID: <child@x>\r\n",
            "References: <parent@x>\r\n",
            "In-Reply-To: <alt@x>\r\n",
            "\r\nbody",
        );
        let raw = raw.as_bytes();
        let msg = MailMessage::from_bytes(raw).unwrap();
        assert!(msg.references.contains(&"parent@x".to_string()));
    }

    #[test]
    fn implements_thread_message_trait() {
        let raw = b"Message-ID: <a@b>\r\nSubject: Test\r\n\r\n.";
        let msg = MailMessage::from_bytes(raw).unwrap();
        let msgs = vec![msg];
        let threads = crate::thread::thread_messages(msgs);
        assert_eq!(threads.len(), 1);
    }
}
