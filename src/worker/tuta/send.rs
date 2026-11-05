//! Sending from a Tuta account. The helper does not take MIME: Tuta builds
//! the mail itself from its parts (sender, recipients, subject, HTML body
//! and attachments), so the message `build_email` produces is read back
//! into those parts. Tuta files the Sent copy itself.
//!
//! Delivery is chosen as Tuta's own app chooses it. A message only to Tuta
//! addresses goes end-to-end encrypted ("confidential"), each recipient's
//! key loaded by the helper through Tuta's SDK. A message that also goes
//! outside Tuta goes as the composer chose: confidential, each outside
//! recipient reading it in Tuta's web app behind a password, or ordinary
//! mail (encrypted in transit only) to everyone, which the user agreed to.
//! A Tuta address the server has no key for is refused, never sent
//! unencrypted instead.
//!
//! The passwords never reach the Outbox: a queued confidential message
//! reads them again from the Tuta contacts the helper keeps them in, and
//! one that finds a recipient without goes back to the composer.
//!
//! A reply or a forward names the message it answers: the helper creates
//! the draft in that message's conversation, as Tuta's own app does, and
//! marks the original replied or forwarded once it went out.

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use mail_parser::{MessageParser, MimeHeaders, PartType};
use serde_json::{json, Value};

use crate::cache::Cache;
use crate::config::AccountConfig;
use crate::i18n::{i18n, i18n_f};
use crate::models::{FolderKind, OutboxItem, TutaDelivery, TutaPasswords};
use crate::worker::compose::build::build_email;
use crate::worker::helper::{HelperClient, HelperError, Object};
use crate::worker::outbox::{due_outbox_items, emit_outbox, uncertain_mark, uncertain_text};
use crate::worker::tuta::index::{run, tuta_load_folder, TutaState};
use crate::worker::tuta::ops::emit_folder;
use crate::worker::tuta::wire::{contact_passwords, password_map};
use crate::worker::{offline_text, OutgoingMessage, WorkerEvent};

/// How a send ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum SendOutcome {
    Sent,
    /// Nothing went out and trying again could go through: the Outbox
    /// keeps it. The text says why.
    Failed(String),
    /// Nothing went out, and the same message would be refused again: it
    /// goes back to the composer. The text says why.
    Refused(String),
    /// [`SendOutcome::Refused`] because a recipient outside Tuta has no
    /// password: a queued message goes back to the composer for one.
    PasswordRequired(String),
    /// It may have gone out. Never sent again on its own.
    Uncertain,
}

/// What a helper failure on a send means for the message.
pub(super) fn send_outcome(error: &HelperError) -> SendOutcome {
    match error.code.as_str() {
        // The helper reached the submit and lost track of it. A helper that
        // died, overran, or answered something unreadable cannot say where
        // it got to either: any of those may have followed the submit.
        "send_uncertain"
        | "helper_failed"
        | "timeout"
        | "invalid_response"
        | "response_too_large"
        | "unsupported_version"
        | "request_failed" => SendOutcome::Uncertain,
        // Decided before anything was written to the server.
        "password_required" => SendOutcome::PasswordRequired(error.message()),
        "recipient_not_found"
        | "secure_external_sender_unsupported"
        | "invalid_recipient"
        | "sender_unknown"
        | "body_too_large"
        | "invalid_request"
        | "invalid_thread"
        | "ordinary_delivery_requires_consent" => SendOutcome::Refused(error.message()),
        // The helper is missing, the sign-in needs renewing, the server was
        // out of reach, or it turned the draft down before the submit: none
        // sent anything, and a later try may well go.
        _ => SendOutcome::Failed(error.message()),
    }
}

fn addresses_of(address: Option<&mail_parser::Address<'_>>) -> Vec<(String, String)> {
    address
        .map(|list| {
            list.iter()
                .filter_map(|a| {
                    let email = a.address.as_deref().filter(|a| !a.is_empty())?;
                    Some((
                        a.name.as_deref().unwrap_or_default().to_string(),
                        email.to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn address_rows(addresses: &[(String, String)]) -> Value {
    addresses
        .iter()
        .map(|(name, address)| json!({"name": name, "address": address}))
        .collect()
}

fn escape_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            _ => out.push(c),
        }
    }
    out
}

/// The HTML Tuta stores: the HTML part, else the text part escaped.
fn send_body(message: &mail_parser::Message<'_>) -> String {
    let html = message
        .html_part(0)
        .filter(|part| matches!(part.body, PartType::Html(_)));
    match html.or_else(|| message.text_part(0)).map(|part| &part.body) {
        Some(PartType::Html(html)) => html.to_string(),
        Some(PartType::Text(text)) => format!("<pre>{}</pre>", escape_html(text)),
        _ => String::new(),
    }
}

fn send_attachments(message: &mail_parser::Message<'_>) -> Result<Vec<Value>, HelperError> {
    message
        .attachments()
        .map(|part| {
            let data: &[u8] = match &part.body {
                PartType::Binary(data) | PartType::InlineBinary(data) => data,
                PartType::Text(text) | PartType::Html(text) => text.as_bytes(),
                // A forwarded message travels as its own bytes.
                PartType::Message(nested) => nested.raw_message(),
                PartType::Multipart(_) => return Err(HelperError::new("attachment_failed")),
            };
            let mime_type = part.content_type().map_or_else(
                || "text/plain".to_string(),
                |kind| match &kind.c_subtype {
                    Some(sub) => format!("{}/{}", kind.c_type, sub).to_ascii_lowercase(),
                    None => kind.c_type.to_ascii_lowercase(),
                },
            );
            let cid = part
                .content_id()
                .map(|cid| cid.trim().trim_matches(['<', '>']))
                .filter(|cid| !cid.is_empty());
            Ok(json!({
                "name": part.attachment_name().filter(|n| !n.is_empty()).unwrap_or("attachment"),
                "mime_type": mime_type,
                "cid": cid,
                "data": STANDARD.encode(data),
            }))
        })
        .collect()
}

/// The parts of a built message the helper takes, with `bcc` from wherever
/// the caller found it. To and Cc keep their names from the headers.
/// Threading goes separately (see [`thread_parameters`]).
fn message_parts(
    message: &mail_parser::Message<'_>,
    bcc: impl FnOnce(&[(String, String)]) -> Vec<(String, String)>,
) -> Result<Object, HelperError> {
    let (sender_name, sender) = addresses_of(message.from())
        .into_iter()
        .next()
        .ok_or_else(|| HelperError::new("sender_unknown"))?;
    let to = addresses_of(message.to());
    let cc = addresses_of(message.cc());
    let named: Vec<(String, String)> = to.iter().chain(&cc).cloned().collect();
    let bcc = bcc(&named);
    let reply_tos = addresses_of(message.reply_to());
    let mut parameters = Object::new();
    for (key, value) in [
        ("sender", Value::from(sender)),
        ("sender_name", sender_name.into()),
        ("to", address_rows(&to)),
        ("cc", address_rows(&cc)),
        ("bcc", address_rows(&bcc)),
        ("reply_tos", address_rows(&reply_tos)),
        ("subject", message.subject().unwrap_or_default().into()),
        ("body_html", send_body(message).into()),
        ("attachments", Value::Array(send_attachments(message)?)),
    ] {
        parameters.insert(key.into(), value);
    }
    Ok(parameters)
}

/// Where a built message sits in a conversation, as the helper's optional
/// `in_reply_to`, `references` and `action`: nothing for new mail. A
/// forward names the message it forwards as its parent, as Tuta's own app
/// does. A reply is a reply-all when it goes to more than one address: the
/// composer keeps no other mark of it. `recipients` are every address the
/// message goes to.
pub(super) fn thread_parameters(
    message: &mail_parser::Message<'_>,
    recipients: &[String],
    forwards: &str,
) -> Object {
    let in_reply_to = sendable_msgids(message.in_reply_to());
    let references = sendable_references(&sendable_msgids(message.references()));
    let forwarded = forwards.split_whitespace().find(|id| sendable_msgid(id));
    let parent = forwarded.or_else(|| in_reply_to.first().map(String::as_str));
    let mut parameters = Object::new();
    if parent.is_none() && references.is_empty() {
        return parameters;
    }
    if let Some(parent) = parent {
        parameters.insert("in_reply_to".into(), parent.into());
    }
    if !references.is_empty() {
        parameters.insert("references".into(), references.into());
    }
    let distinct: std::collections::HashSet<String> =
        recipients.iter().map(|a| a.to_lowercase()).collect();
    let action = match (forwarded, distinct.len()) {
        (Some(_), _) => "forward",
        (None, n) if n > 1 => "reply_all",
        (None, _) => "reply",
    };
    parameters.insert("action".into(), action.into());
    parameters
}

/// Whether the helper takes `id` as a Message-ID: it refuses the whole
/// request over one it does not, and a reply must not fail over a malformed
/// id some other client wrote into the thread.
fn sendable_msgid(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 998
        && id
            .bytes()
            .all(|b| b.is_ascii_graphic() && b != b'<' && b != b'>')
}

/// The well-formed Message-IDs of an In-Reply-To or References header,
/// without their brackets.
fn sendable_msgids(header: &mail_parser::HeaderValue) -> Vec<String> {
    let ids: Vec<&str> = match header {
        mail_parser::HeaderValue::Text(id) => vec![id.as_ref()],
        mail_parser::HeaderValue::TextList(ids) => ids.iter().map(|id| id.as_ref()).collect(),
        _ => Vec::new(),
    };
    ids.into_iter()
        .map(|id| id.trim().trim_start_matches('<').trim_end_matches('>'))
        .filter(|id| sendable_msgid(id))
        .map(str::to_string)
        .collect()
}

/// The References chain as the helper takes it: the newest 100 ids within
/// 16 KiB, oldest first.
fn sendable_references(ids: &[String]) -> String {
    let mut kept: Vec<&str> = Vec::new();
    let mut size = 0;
    for id in ids.iter().rev().take(100) {
        size += id.len() + 1;
        if size > 16 * 1024 {
            break;
        }
        kept.push(id);
    }
    kept.reverse();
    kept.join(" ")
}

/// The forwarded message's Message-ID as it arrived: the cache keeps it
/// lowercased for matching, and the helper finds the parent by its exact
/// id (as `restore_msgid_case` does for the threading headers).
pub(super) fn exact_forwards(cache: Option<&Cache>, forwards: &str) -> String {
    cache.map_or_else(|| forwards.to_string(), |c| c.exact_msgids(forwards))
}

fn parse(raw: &[u8]) -> Result<mail_parser::Message<'_>, HelperError> {
    MessageParser::default()
        .parse(raw)
        .ok_or_else(|| HelperError::new("invalid_request"))
}

/// The helper's `send` parameters for a built message, its envelope and
/// how it is to go. Bcc is only in the envelope, so it is the envelope
/// less To and Cc. `forwards` is [`exact_forwards`] of the message's
/// `forwards`.
pub(super) fn send_parameters(
    raw: &[u8],
    envelope: &[String],
    forwards: &str,
    delivery: &TutaDelivery,
) -> Result<Object, HelperError> {
    let message = parse(raw)?;
    let mut parameters = message_parts(&message, |named| {
        envelope
            .iter()
            .filter(|address| !named.iter().any(|(_, a)| a.eq_ignore_ascii_case(address)))
            .map(|address| (String::new(), address.clone()))
            .collect()
    })?;
    // The helper decides who is outside Tuta. Confidential with someone
    // outside needs each one's password, and is refused rather than sent
    // without; not confidential with someone outside needs the user's
    // consent, and is refused without it.
    parameters.insert("confidential".into(), delivery.confidential.into());
    parameters.insert(
        "ordinary_confirmed".into(),
        delivery.ordinary_confirmed.into(),
    );
    if delivery.confidential && !delivery.passwords.is_empty() {
        parameters.extend(password_map(&delivery.passwords));
    }
    parameters.extend(thread_parameters(&message, envelope, forwards));
    Ok(parameters)
}

/// The helper's `save_draft` parameters for a built draft, which keeps its
/// Bcc in a header, as a draft is the user's own copy, and whether it is
/// to go confidential, so Tuta's own apps open it the same way. A draft
/// never carries a password.
///
/// `forwards` is [`exact_forwards`] of the message's `forwards`.
pub(super) fn draft_parameters(
    raw: &[u8],
    forwards: &str,
    confidential: bool,
) -> Result<Object, HelperError> {
    let message = parse(raw)?;
    let mut parameters = message_parts(&message, |_| addresses_of(message.bcc()))?;
    parameters.insert("confidential".into(), confidential.into());
    let recipients: Vec<String> = [message.to(), message.cc(), message.bcc()]
        .into_iter()
        .flat_map(addresses_of)
        .map(|(_, address)| address)
        .collect();
    parameters.extend(thread_parameters(&message, &recipients, forwards));
    Ok(parameters)
}

/// Send built bytes. Any reply with a result means the mail went. Its
/// fields are the server's ids, which nothing here needs; whether it went
/// confidentially, with each recipient's key version and fingerprint as
/// the helper loaded them (Tuta verifies no key unless the user has pinned
/// it, and Hylki pins none yet, so those only go to the debug log); and
/// whether the passwords reached the Tuta contacts.
pub(super) async fn tuta_send_raw(
    client: HelperClient,
    raw: Vec<u8>,
    envelope: Vec<String>,
    forwards: String,
    delivery: TutaDelivery,
) -> SendOutcome {
    let sent = run(move || {
        let parameters = send_parameters(&raw, &envelope, &forwards, &delivery)?;
        client.request("send", parameters)
    })
    .await;
    match sent {
        Ok(reply) => {
            let keys = reply
                .get("recipients")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            let outside = reply
                .get("secure_external")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            match reply.get("confidential").and_then(Value::as_bool) {
                Some(true) => tracing::debug!(
                    "tuta: sent end-to-end encrypted to {keys} key(s) and {outside} password(s)"
                ),
                _ => tracing::debug!("tuta: sent as ordinary mail"),
            }
            // The mail went; only the next message to these recipients
            // misses the passwords, and asks for them again.
            if reply.get("contacts_saved").and_then(Value::as_bool) == Some(false) {
                tracing::warn!("tuta: sent, but the passwords were not saved to the contacts");
            }
            SendOutcome::Sent
        }
        Err(e) => {
            if e.code != "send_uncertain" {
                tracing::warn!("tuta: send failed: {}", e.code);
            }
            send_outcome(&e)
        }
    }
}

/// Send a composed message. `forwards` is [`exact_forwards`] of its
/// `forwards`.
pub(super) async fn tuta_send_message(
    account: &AccountConfig,
    state: &TutaState,
    message: &OutgoingMessage,
    forwards: String,
) -> SendOutcome {
    // Tuta rebuilds the mail from its parts, which would leave an OpenPGP
    // signature or encryption behind: refused, not sent without it.
    if message.sign || message.encrypt {
        return SendOutcome::Refused(i18n(
            "OpenPGP signing and encryption aren't available for Tuta accounts. Turn them off to send.",
        ));
    }
    let email = match build_email(account, message) {
        Ok(email) => email,
        Err(e) => return SendOutcome::Failed(e.to_string()),
    };
    if !crate::desktop::power::reaches_internet() {
        return SendOutcome::Failed(offline_text());
    }
    let client = match state.client(account) {
        Ok(client) => client,
        Err(e) => return SendOutcome::Failed(e.message()),
    };
    let envelope = email
        .envelope()
        .to()
        .iter()
        .map(|a| a.to_string())
        .collect();
    tuta_send_raw(
        client,
        email.formatted(),
        envelope,
        forwards,
        message.tuta.clone(),
    )
    .await
}

/// List the Sent folder, where Tuta filed the copy, so it joins the
/// conversation it answers (#199).
pub(super) async fn tuta_list_sent(
    account: &AccountConfig,
    account_id: u32,
    cache: Option<&Cache>,
    state: &mut TutaState,
    emit: &impl Fn(WorkerEvent),
) {
    let Some(path) = state.path_of(FolderKind::Sent) else {
        return;
    };
    let Some((folder_id, _)) = state.folders.get(&path).cloned() else {
        return;
    };
    if let Ok(messages) =
        tuta_load_folder(account, account_id, folder_id, &path, cache, state, emit).await
    {
        emit_folder(folder_id, messages, state, emit);
    }
}

/// The items a flush may send: never one whose last send may have gone
/// out. Asked for by id (Send Now), such an item is said to be held back.
fn sendable(
    items: Vec<OutboxItem>,
    id: Option<u32>,
    emit: &impl Fn(WorkerEvent),
) -> Vec<OutboxItem> {
    let (held, sendable): (Vec<_>, Vec<_>) =
        items.into_iter().partition(OutboxItem::is_send_uncertain);
    if id.is_some() && !held.is_empty() {
        emit(WorkerEvent::error(i18n(
            "This message may already have been sent, so it is not sent again from the Outbox. Check Sent; to send it anyway, open it and send it from the composer.",
        )));
    }
    sendable
}

/// How a queued message goes: as it was queued, and, when confidential,
/// with the passwords the user's Tuta contacts keep for its recipients,
/// read now since the Outbox keeps none. Which recipients are outside Tuta
/// is the helper's to say again at the send.
async fn queued_delivery(
    client: &HelperClient,
    item: &OutboxItem,
) -> Result<TutaDelivery, HelperError> {
    let mut delivery = item.delivery.clone();
    if !delivery.confidential {
        return Ok(delivery);
    }
    let lookup = client.clone();
    let rcpts = item.rcpts.clone();
    let recipients = run(move || contact_passwords(&lookup, &rcpts)).await?;
    let mut passwords = TutaPasswords::default();
    for r in recipients.iter().filter(|r| r.external != Some(false)) {
        if let Some(password) = &r.password {
            passwords.set(&r.address, password);
        }
    }
    delivery.passwords = passwords;
    Ok(delivery)
}

/// The Outbox for a Tuta account: each due message goes as it was queued.
/// One that may have gone out is marked and kept, never retried. One that
/// is confidential and finds an outside recipient without a password in
/// the contacts stays queued, and opens in the composer for it.
pub(super) async fn tuta_flush_outbox(
    cache: Option<&Cache>,
    account_id: u32,
    account: &AccountConfig,
    id: Option<u32>,
    state: &mut TutaState,
    emit: &impl Fn(WorkerEvent),
) {
    let Some(c) = cache else { return };
    let items = sendable(due_outbox_items(c, account_id, id), id, emit);
    if items.is_empty() {
        emit_outbox(cache, account_id, emit);
        return;
    }
    let client = match state.client(account) {
        Ok(client) => client,
        Err(e) => {
            emit(WorkerEvent::error(e.message()));
            return;
        }
    };
    emit(WorkerEvent::Status(i18n("Sending…")));
    let mut sent_any = false;
    for item in items {
        let forwards = exact_forwards(cache, &item.forwards);
        let outcome = match queued_delivery(&client, &item).await {
            Ok(delivery) => {
                tuta_send_raw(
                    client.clone(),
                    item.raw.clone(),
                    item.rcpts.clone(),
                    forwards,
                    delivery,
                )
                .await
            }
            Err(e) => {
                tracing::warn!(
                    "[account {account_id}] could not read the passwords of queued message {} from the Tuta contacts: {}",
                    item.id,
                    e.code
                );
                SendOutcome::Failed(e.message())
            }
        };
        match outcome {
            SendOutcome::Sent => {
                sent_any = true;
                c.delete_outbox(item.id);
            }
            SendOutcome::Uncertain => {
                let why = uncertain_text();
                c.record_outbox_failure(item.id, &uncertain_mark(&why));
                emit(WorkerEvent::error(i18n_f(
                    "“{subject}”: {e}",
                    &[("subject", &item.subject), ("e", &why)],
                )));
                break;
            }
            SendOutcome::PasswordRequired(e) => {
                c.record_outbox_failure(item.id, &e);
                emit(WorkerEvent::error(i18n_f(
                    "Could not send “{subject}”: {e}",
                    &[("subject", &item.subject), ("e", &e)],
                )));
                emit(WorkerEvent::EditQueued { id: item.id });
                break;
            }
            SendOutcome::Failed(e) | SendOutcome::Refused(e) => {
                c.record_outbox_failure(item.id, &e);
                emit(WorkerEvent::error(i18n_f(
                    "Still could not send “{subject}”: {e}",
                    &[("subject", &item.subject), ("e", &e)],
                )));
                break;
            }
        }
    }
    emit(WorkerEvent::Status(String::new()));
    if sent_any {
        tuta_list_sent(account, account_id, cache, state, emit).await;
        emit(WorkerEvent::Sent);
    }
    emit_outbox(cache, account_id, emit);
}
