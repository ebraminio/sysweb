# Lineage: from pad.js to sysweb

`sysweb` is a spiritual continuation of [pad.js](https://github.com/ebraminio/pad.js),
a single-file Node.js file-transfer server. This document records what that
means concretely: which ideas are inherited, which are adapted, and which are
deliberately left behind.

## pad.js in one paragraph

pad.js was a “total fancy node.js webserver for transferring files from/to
browser console and terminal.” It was distributed as one `.html` file that was
also a valid Node program. Running it gave you:

- a directory index with names, sizes, dates, types, and client-side sorting;
- downloads with `Range` support and permissive CORS;
- uploads, folder creation, notes, delete, and rename (dangerous operations
  opt-in);
- optional Basic auth, HTTPS, a path prefix, inactivity timeout, and command
  execution;
- a landing page with terminal and browser-console recipes;
- zero npm dependencies, zero build step, zero install: `curl pad.js.org | node`.

It was a pocket file-transfer appliance disguised as a web page.

## The file trick

The first line is `<!--`. JavaScript treats `<!--` as an HTML-like comment
(Annex B), so Node ignores it. HTML treats it as the start of a comment, so the
browser ignores all the JavaScript that follows.

Near the end, the JavaScript opens a block comment and closes the HTML comment
on the same line:

```html
<!--
'use strict';
var http = require('http');
// ... the whole server ...
/* -->
<!doctype html>
<title>pad.js</title>
<!-- */
```

- **Node** sees `<!--` as a line comment, runs the JavaScript, then sees
  `/* -->` which starts a block comment containing the landing-page HTML, and
  closes it at `*/`.
- **The browser** sees `<!--` and ignores everything until `-->` on the
  `/* -->` line, then renders the landing page, and treats the final `<!--` as
  another comment.

One file is the installer, the program, the documentation, and the landing
page. There is no bundle, no build step, and no asset directory to lose.

## What sysweb inherits

1. **One self-contained artifact.** A sysweb release is one static binary.
   The directory UI, CSS, and JavaScript are compiled into it. Nothing is
   fetched at runtime, no `node_modules`, no CDN, no public/ asset pipeline is
   required for the UI. The binary is the distribution.
2. **Zero dependencies.** pad.js used only Node built-ins; sysweb uses only
   `core` and compiler builtins. `cargo tree` has one node.
3. **A pocket transfer appliance.** Start it in a directory and immediately
   browse, download, and (opt-in) upload. The default bind is loopback, like
   `pad --local`.
4. **A built-in, useful index.** Names, sizes, modification times, types,
   breadcrumbs, sorting, and a browser upload control. No generated asset
   files: the page is built on demand.
5. **Browser *and* terminal workflows.** The same server is meant to be used
   from a drag-and-drop UI and from `curl`, exactly like pad.js’s showcases.
6. **Dangerous features are explicit.** Uploads are off by default here (pad.js
   defaulted them on); `--replace` and future delete/rename are explicit.
   `--no-index` still exists.
7. **A human landing page.** The README is the landing page; the directory
   index is the in-product help. Terminal recipes are shown in the UI.

## What sysweb adapts

| pad.js | sysweb |
| --- | --- |
| Node runtime, one `.html` file | Static Rust binary, no runtime |
| `require('http')`, Node streams | Raw syscalls behind a `Platform` trait |
| Heap, buffers, arrays, callbacks | Fixed buffers, no allocator, blocking loop |
| Multipart upload parser | Raw `PUT` primary; bounded `multipart/form-data` `POST` for classic forms |
| Directory HTML built from arrays | 64 KiB fixed HTML buffer, streamed escaping/encoding |
| `fs.readdir`, `fs.stat` | `getdents64`/`readdir`, `statx`/`fstatat` |
| Follows links unless checked | All symlinks beneath the root denied |
| HTTPS in-process | TLS out of scope; put a reverse proxy in front |
| `--cmd` command execution | Never implemented; remote code execution is not a feature |
| `config.json` | CLI flags; a config file is roadmap |

Raw `PUT` is still the primary path because it is simpler and supports
progress cleanly. Classic `multipart/form-data` `POST` is now implemented as a
bounded compatibility layer for old and no-JavaScript browsers, using a 32 KiB
streaming parser and one file part per request:

```sh
curl -T report.pdf http://127.0.0.1:8080/files/report.pdf
```

```js
await fetch('/files/report.pdf', { method: 'PUT', body: file });
```

The built-in UI uses `XMLHttpRequest` for progress and a `PUT` body, and also
contains a `<noscript>` multipart form for browsers without JavaScript.

## What is deliberately not inherited

- **`--cmd`.** Executing shell commands from an HTTP endpoint is a remote code
  execution service. It is not on the roadmap.
- **In-process HTTPS.** A `no_std` TLS stack is a separate project. Terminate
  TLS at a reverse proxy.
- **Silent write access.** pad.js was convenience-first; sysweb defaults to
  read-only and loopback. `--upload` is an explicit decision.
- **Unbounded request bodies.** Raw uploads are capped at 1 GiB and require
  `Content-Length`; chunked bodies are rejected.
- **Multipart as the primary protocol.** It is supported only as a bounded
  compatibility path (one file per request), not as the main wire format.

## Feature map

| pad.js capability | sysweb now | next |
| --- | --- | --- |
| Browse / download | ✅ | — |
| Built-in directory index | ✅ | — |
| Size / date / type / sort | ✅ | — |
| Breadcrumbs / parent link | ✅ | — |
| Upload from browser | ✅ with `--upload` (`PUT`/XHR + no-JS form) | resumable uploads |
| Upload from terminal (`curl -T` / `curl -F`) | ✅ with `--upload` | `--max-upload` config |
| Classic multipart form | ✅ with `--upload`, one file | multi-file forms, `filename*` |
| Create folder (`PUT .../`) | ✅ with `--upload` | — |
| Replace existing file | ✅ with `--replace` | atomic temp+rename |
| Delete | ❌ | opt-in `--delete` |
| Rename / move | ❌ | opt-in, two-step, containment-checked |
| Basic auth | ❌ | `--userpass` |
| CORS / cross-origin isolation | ❌ | opt-in headers |
| Range downloads | ✅ single range; multi-range ignored | — |
| Conditional GET (`ETag`) | ❌ | milestone 1 |
| Inactivity timeout | ❌ | milestone 1 |
| Port fallback | ✅ when the default port is implicit | — |
| Path prefix | ❌ | milestone 1 |
| Command execution | ❌ by design | never |
| HTTPS | ❌ | reverse proxy |

## The continuation thesis

pad.js proved that a file-transfer tool does not need an installer, a
framework, or a build step to be pleasant. sysweb asks the next question:

> Can the same experience be a single static binary with no runtime, no
> allocator, no libc on Linux, and direct OS interfaces underneath?

The answer for the first slice is yes: one ~40 KiB Linux binary serves a
directory, renders a self-contained UI, downloads files, accepts uploads, and
creates folders, while the HTTP and path code stays safe Rust and the unsafe
code stays in the syscall layer.

The destination is a “pocket appliance” you can copy to a machine over SSH,
run, use from a browser or terminal, and delete. No Node, no Python, no package
manager, no configuration file, no daemon manager required.

## Reading order

- [README.md](README.md) — quick start and current behavior
- [ARCHITECTURE.md](ARCHITECTURE.md) — the syscall and buffer design
- [ROADMAP.md](ROADMAP.md) — parity and async milestones
- [SECURITY.md](SECURITY.md) — write access, symlinks, and exposure
