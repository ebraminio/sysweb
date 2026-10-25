# Old-browser chat spike

A small local-network chat experiment with **no WebSocket**. The baseline works
without JavaScript; modern browsers get incremental polling instead of page
reloads.

The form lives in the page. The message list lives in an iframe. A message POST
is answered with `303 See Other`, so refreshing the form never resubmits it.

This is a protocol/UI prototype, not production code. It uses std threads and a
heap mutex; a sysweb version would keep the ring in `run` and use fixed slots.

## Run

```sh
cargo build --release --manifest-path spikes/chat/Cargo.toml
spikes/chat/target/release/chat-spike --bind 0.0.0.0 --port 19550
```

Open:

```text
http://127.0.0.1:19550/
```

## Protocol

```text
GET /                     form + iframe
GET /messages             full message page
GET /messages?pause=1     no auto-refresh; copy-friendly
GET /messages?since=N     HTML fragment for N+1..; X-Since header
POST /chat                text=...; prepend; 303 Location: /
```

No history is promised. The server keeps a fixed ring of **16 messages × 400
bytes**, plus the peer IP address, and clears it on restart.

## The page

```html
<form method="post" action="/chat">
  <textarea name="text" rows="4" maxlength="400"></textarea>
  <button>Send</button>
</form>
<iframe src="/messages"></iframe>
```

The form and the message list are separate, so a message update does not erase
half-typed input.

Messages are newest-first. The sender address is on its own line and the text
is indented beneath it:

```html
<div class="line">
  <div class="from">127.0.0.1</div>
  <div class="text">line one<br>line two</div>
</div>
```

When JavaScript is available, **Enter** submits and **Ctrl+Enter** inserts a
newline. Without JavaScript, the textarea keeps its normal Enter behavior and
the Send button submits.

## Copying text

`<meta http-equiv="refresh">` replaces the document and therefore destroys the
browser selection. This spike handles that in two ways:

- `/messages?pause=1` omits the refresh and shows a “Resume live updates” link;
- modern browsers run a small polling script and remove the meta refresh.

The polling script fetches `/messages?since=N`, inserts only new messages at
the top, and **skips the update while there is a text selection**. Previously
rendered messages are not replaced, so a selection can survive new messages
arriving.

Without JavaScript, pause first, then copy.

## Why polling, not long-poll

A long-held request or SSE stream consumes one connection slot per client. With
sysweb's 32 slots, that is effectively a 32-client cap. Polling uses a slot for
milliseconds; 100 clients at one refresh per second is only ~100 requests per
second.

The spike uses one thread per request for simplicity. A sysweb chat mode would
use the existing reactor and the same short polling responses.

## What a sysweb chat mode would need

- a `--chat` mode with routes `/`, `/messages`, `/chat`;
- shared `Chat` state in `run`, not per-slot;
- a URL-encoded form body parser and multiline text preservation;
- a `ReadingChatBody` state for split POST bodies;
- streamed or size-capped page rendering, because 16 messages can exceed the
  4.6 KiB response buffer if the text limit is raised;
- HTML escaping, a message-length cap, and the peer IP;
- sequence numbers for the `since` fragment;
- no persistence and no WebSocket;
- newest-first rendering, sender on its own line, and indented text;
- Basic auth via `--userpass` if desired.

The interesting part is that the old-browser UX is entirely HTTP: a form, a
303, an iframe, a meta refresh, and a pause link.

## Try it

```sh
curl http://127.0.0.1:19550/
curl http://127.0.0.1:19550/messages
curl -i -X POST --data 'text=hello%20%3Cworld%3E' http://127.0.0.1:19550/chat
curl http://127.0.0.1:19550/messages
curl -i 'http://127.0.0.1:19550/messages?since=1'
```

The escaped message appears as `hello &lt;world&gt;`. The fragment response
includes `X-Since`, the latest sequence number.

The address is the TCP peer, not an authenticated identity. Behind a reverse
proxy this would need `X-Forwarded-For` handling, which the spike does not do.
