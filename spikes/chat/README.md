# Old-browser chat spike

A small local-network chat experiment with **no WebSocket**. It uses XHR
polling, which is old enough to cover most browsers that can do background
requests at all.

The form lives in the page and the message list lives in an iframe. A message
POST is answered with `303 See Other`, so refreshing never resubmits it.

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

The form and the message list are separate, so message updates do not erase
half-typed input.

Messages are newest-first. The sender address is on its own line above an
iOS-style bubble:

```html
<div class="line" data-seq="2" data-from="127.0.0.1">
  <div class="from">127.0.0.1</div>
  <div class="bubble">line one<br>line two</div>
</div>
```

When JavaScript is available, the iframe compares `data-from` with its own
`data-me` peer address and aligns messages from that address to the right in a
blue bubble. Other senders remain on the left.

The form uses a rounded textarea and a blue send button. The textarea’s CRLF is
normalized to a single `<br>`, so a multiline message does not get a double line
break.

Both pages declare `color-scheme: light dark` and use a
`prefers-color-scheme: dark` override like the sysweb listing page.

When JavaScript is available, **Enter** submits and **Ctrl+Enter** inserts a
newline. Without JavaScript, the textarea keeps its normal Enter behavior and
the Send button submits.

## Updates and copying

`/messages` has no meta refresh and no pause link. A small XHR loop does:

```text
GET /messages?since=N
```

It prepends only new messages and leaves the rest of the DOM alone. If there is
a text selection, the update is skipped until the selection is cleared, so
copying survives new messages arriving.

The code uses `XMLHttpRequest`, `onreadystatechange`, `insertAdjacentHTML`, and
a `document.selection` fallback rather than `fetch`/Promises.

Without JavaScript there is no automatic update; the page shows:

```html
<noscript>Automatic updates need JavaScript. <a href="/messages">Refresh</a>.</noscript>
```

So the no-JS view is static and copyable, with manual refresh.

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
- newest-first rendering, sender on its own line, and iOS-style bubbles;
- own-address detection for right-aligned messages;
- Basic auth via `--userpass` if desired.

The old-browser UX is still entirely HTTP: a form, a 303, an iframe, and XHR
fragments.

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
