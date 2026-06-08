//! 1:1 "calls" over a direct QUIC connection on a custom ALPN.
//!
//! A call is a direct, contact-gated message exchange: the caller dials the
//! callee's endpoint on `CALL_ALPN`, sends a short message, and reads an ack.
//! The callee only accepts the connection if the caller is in its contacts.

use std::sync::Arc;

use anyhow::Result;
use iroh::{
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler},
    Endpoint, EndpointId,
};
use serde::{Deserialize, Serialize};

use crate::{control::EventKind, node::Shared};

/// ALPN for the 1:1 call protocol.
pub const CALL_ALPN: &[u8] = b"groupchat/call/0";

const MAX_CALL_BYTES: usize = 64 * 1024;

#[derive(Debug, Serialize, Deserialize)]
struct CallMsg {
    nick: String,
    text: String,
}

/// Inbound call handler. Rejects callers that are not contacts.
#[derive(Debug)]
pub struct CallHandler {
    shared: Shared,
}

impl CallHandler {
    pub fn new(shared: Shared) -> Arc<Self> {
        Arc::new(Self { shared })
    }
}

impl ProtocolHandler for CallHandler {
    async fn accept(&self, connection: Connection) -> std::result::Result<(), AcceptError> {
        let remote = connection.remote_id();
        let is_contact = self.shared.contacts.lock().unwrap().contains(&remote);

        let (mut send, mut recv) = connection.accept_bi().await?;

        if !is_contact {
            let ack = CallMsg {
                nick: self.shared.nick.clone(),
                text: "call rejected: you are not in their contacts".to_string(),
            };
            let bytes = serde_json::to_vec(&ack).map_err(AcceptError::from_err)?;
            send.write_all(&bytes).await.map_err(AcceptError::from_err)?;
            send.finish()?;
            connection.closed().await;
            return Ok(());
        }

        let req_bytes = recv
            .read_to_end(MAX_CALL_BYTES)
            .await
            .map_err(AcceptError::from_err)?;
        let msg: CallMsg = serde_json::from_slice(&req_bytes).map_err(AcceptError::from_err)?;

        let nick = self
            .shared
            .contacts
            .lock()
            .unwrap()
            .nick_of(&remote)
            .unwrap_or_else(|| msg.nick.clone());
        self.shared.events.lock().unwrap().push_direct(
            EventKind::Call,
            remote.to_string(),
            nick,
            format!("\u{1F4DE} incoming call: {}", msg.text),
        );

        let ack = CallMsg {
            nick: self.shared.nick.clone(),
            text: "call accepted".to_string(),
        };
        let bytes = serde_json::to_vec(&ack).map_err(AcceptError::from_err)?;
        send.write_all(&bytes).await.map_err(AcceptError::from_err)?;
        send.finish()?;
        connection.closed().await;
        Ok(())
    }
}

/// Place an outbound call: dial the target on `CALL_ALPN` and exchange a message.
/// Returns the callee's ack text.
pub async fn place_call(
    endpoint: &Endpoint,
    target: EndpointId,
    nick: &str,
    text: &str,
) -> Result<String> {
    let conn = endpoint.connect(target, CALL_ALPN).await?;
    let (mut send, mut recv) = conn.open_bi().await?;

    let req = CallMsg {
        nick: nick.to_string(),
        text: text.to_string(),
    };
    send.write_all(&serde_json::to_vec(&req)?).await?;
    send.finish()?;

    let resp_bytes = recv.read_to_end(MAX_CALL_BYTES).await?;
    let resp: CallMsg = serde_json::from_slice(&resp_bytes)?;
    conn.close(0u32.into(), b"done");
    Ok(resp.text)
}
