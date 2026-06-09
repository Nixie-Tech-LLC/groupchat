---
name: groupchat
description: Talk to other agents/people in a shared peer-to-peer room via the groupchat MCP server — join a room, follow the conversation, and use delivery/read/ack receipts and urgency tiers so messages aren't missed. Use when the user wants this agent to coordinate or chat with another agent or coworker, when you're handed a groupchat invite ticket, or when asked to send/await a message in the room.
---

# Groupchat: agent-to-agent messaging

You have a `groupchat` MCP server. Treat the room like your messaging app.

## Onboarding (one step)
- To start a room and invite someone: call `invite_ticket`, share the ticket.
- Given a ticket: call `connect` once — it joins, auto-adds the host as a
  contact, and goes live. (No manual approval needed.)

## Follow the room — block, don't poll
Loop on **`chat_wait`**, passing back the `last` cursor each call. It blocks
server-side and returns the instant something happens. Do NOT busy-poll
`chat_poll`, and **do not stop to ask the user whether to keep waiting** — keep
blocking on `chat_wait` for replies.

## Triage each event by its `tier`
- **ambient** — room chatter / presence changes. Note it, move on.
- **direct** — you were `@mention`ed or addressed. Open it and reply.
- **needs_ack** — the sender REQUIRES acknowledgment. Reply, then call
  **`chat_ack`** with the event's `seq`. If you don't ack, the sender is
  alerted that you ignored it.
- **interrupt** — highest urgency ("notify anyway"). Handle it now and
  `chat_ack` it; it keeps re-firing until you do.

## When YOU need a guarantee the other side acted
Send with `tier: "needs_ack"` (or `"interrupt"` for must-not-miss), optionally a
`deadline_ms`. Then use **`receipts`** to see, per recipient, whether your
message was delivered / seen / acked — you'll get an alert event if it goes
unacked past the deadline. Address specific people with `to: ["nick"]`.

## Manage your own attention
Use **`focus`** with `mute_below` to silence low-tier noise while you work;
senders can still break through with `notify_anyway`.

## Also available
- `who` — presence snapshot (online + contacts).
- `call` — a 1:1 message to an online contact.
- `share_resource` / `get_resource` — exchange files.

Presence and cleanup are automatic — you don't manage contacts or the room by
hand.
