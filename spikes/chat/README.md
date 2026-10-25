# Old-browser chat spike

A small local-network chat experiment with **no WebSocket and no JavaScript**.

The form lives in the page. The message list lives in an iframe whose page uses
`<meta http-equiv="refresh">`. A message POST is answered with `303 See Other`,
so refreshing the form never resubmits it.

This is a protocol/UI prototype, not production code. It uses std threads and a
heap mutex; the sysweb version would keep the ring in `run` and use fixed slots.

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
GET /           form + iframe
GET /messages   last messages; meta refresh every second
POST /chat      text=...; append; 303 Location: /
```

No history is promised. The server keeps a fixed ring of **16 messages × 400
bytes**, plus the peer IP address, and clears it on restart.

The page has no JavaScript:

```html
<form method="post" action="/chat">
  <textarea name="text" rows="4" maxlength="400"></textarea>
  <button>Send</button>
</form>
<iframe src="/messages"></iframe>
```

`/messages` contains:

```html
<meta http-equiv="refresh" content="1">
```

The form and the message list are separate, so the refresh does not erase
half-typed input. This works in browsers older than `fetch`, WebSocket, or
`XMLHttpRequest`.

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
- no persistence and no WebSocket;
- newest-first rendering, sender on its own line, and indented text;
- Basic auth via `--userpass` if desired.

The interesting part is that the old-browser UX is entirely HTTP: a form, a
303, an iframe, and a meta refresh.

## Try it

```sh
curl http://127.0.0.1:19550/
curl http://127.0.0.1:19550/messages
curl -i -X POST --data 'text=hello%20%3Cworld%3E' http://127.0.0.1:19550/chat
curl http://127.0.0.1:19550/messages
```

The escaped message appears as `hello &lt;world&gt;`.

Multiline input is preserved as `<br>` in the message iframe. The source IP is
recorded from `peer_addr()` and shown before the text:

```html
<span class="from">192.168.1.5</span>line one<br>line two
```

The address is the TCP peer, not an authenticated identity. Behind a reverse
proxy this would need `X-Forwarded-For` handling, which the spike does not do.
