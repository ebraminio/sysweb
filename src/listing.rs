//! Built-in directory index, in the spirit of pad.js: a self-contained page
//! with inline CSS/JS, file metadata, sorting, and optional uploads.
//!
//! Rendering is allocation-free. The whole page is built in a caller-provided
//! `FixedBuf`; if a directory is too large the listing is truncated cleanly.

use crate::fixedbuf::FixedBuf;
use crate::mime;
use crate::path::MAX_PATH;
use crate::platform::{DirEntry, EntryKind, Platform};

pub const LISTING_BUF: usize = 64 * 1024;

const CSS: &[u8] = br###"
:root {
  color-scheme: light dark;
  --bg: #f5f6f8; --fg: #17191d; --mut: #5f6672; --line: #d9dde4;
  --panel: #ffffff; --accent: #0b63b6; --hover: #eef3f9; --danger: #a3261e;
}
@media (prefers-color-scheme: dark) {
  :root {
    --bg: #101216; --fg: #e7eaf0; --mut: #9aa3b2; --line: #272c35;
    --panel: #171a20; --accent: #7bb8ff; --hover: #1d232d; --danger: #ff8b80;
  }
}
* { box-sizing: border-box; }
body {
  margin: 0; background: var(--bg); color: var(--fg);
  font: 14px/1.55 ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
}
main { max-width: 78rem; margin: 0 auto; padding: 1.1rem 1.2rem 3rem; }
header {
  display: flex; align-items: baseline; gap: .85rem; flex-wrap: wrap;
  max-width: 78rem; margin: 0 auto; padding: 1rem 1.2rem 0;
}
.brand { font-weight: 700; color: var(--accent); text-decoration: none; letter-spacing: .02em; }
.crumb { color: var(--mut); overflow-wrap: anywhere; }
.crumb a { color: var(--fg); text-decoration: none; }
.crumb a:hover { color: var(--accent); text-decoration: underline; }
.sep { color: var(--mut); padding: 0 .1rem; }
.here { color: var(--fg); }
h1 { font-size: 1.05rem; font-weight: 600; margin: .2rem 0 1rem; overflow-wrap: anywhere; }
h1 .path { color: var(--mut); font-weight: 400; }
.tools { display: flex; gap: .55rem; flex-wrap: wrap; align-items: center; margin: 0 0 .9rem; }
.btn {
  display: inline-flex; align-items: center; gap: .4rem; cursor: pointer;
  border: 1px solid var(--line); background: var(--panel); color: var(--fg);
  border-radius: .45rem; padding: .42rem .7rem; font: inherit;
}
.btn:hover { background: var(--hover); border-color: var(--accent); }
.btn.primary { border-color: var(--accent); color: var(--accent); }
.btn input[type=file] { max-width: 13rem; font: inherit; color: var(--mut); }
#status { color: var(--mut); overflow-wrap: anywhere; }
#status:empty { display: none; }
#drop {
  display: none; border: 1px dashed var(--line); border-radius: .55rem;
  padding: .9rem 1rem; margin: 0 0 1rem; color: var(--mut); text-align: center;
}
#drop.over { border-color: var(--accent); background: var(--hover); color: var(--accent); }
.js-only { display: none; }
.note { color: var(--mut); margin: 0 0 1rem; }
.note code { color: var(--accent); }
table { width: 100%; border-collapse: collapse; background: var(--panel); border: 1px solid var(--line); border-radius: .55rem; overflow: hidden; }
th, td { text-align: left; padding: .5rem .7rem; border-bottom: 1px solid var(--line); vertical-align: top; }
tr:last-child td { border-bottom: 0; }
th { color: var(--mut); font-weight: 500; white-space: nowrap; }
th button { background: none; border: 0; color: inherit; font: inherit; cursor: pointer; padding: 0; }
th button:hover { color: var(--accent); text-decoration: underline; }
tbody tr:hover { background: var(--hover); }
td.name { overflow-wrap: anywhere; }
td.name a { color: var(--fg); text-decoration: none; }
td.name a:hover { color: var(--accent); text-decoration: underline; }
td.name .symlink { color: var(--mut); }
td.size, td.type { white-space: nowrap; color: var(--mut); }
td.size { text-align: right; font-variant-numeric: tabular-nums; }
td.modified { white-space: nowrap; color: var(--mut); }
tr.parent td.name a { color: var(--accent); }
tr.empty td { color: var(--mut); text-align: center; padding: 1.4rem; }
footer { color: var(--mut); margin-top: 1.4rem; font-size: .92em; }
footer a { color: var(--accent); }
details { margin-top: 1rem; }
summary { cursor: pointer; color: var(--mut); }
pre { background: var(--panel); border: 1px solid var(--line); border-radius: .45rem; padding: .7rem .8rem; overflow-x: auto; }
@media (max-width: 760px) {
  th:nth-child(2), td:nth-child(2), th:nth-child(4), td:nth-child(4) { display: none; }
  th, td { padding: .45rem .5rem; }
  .btn input[type=file] { max-width: 9rem; }
}
"###;

const JS: &[u8] = br###"
(function () {
  'use strict';
  var body = document.body;
  if (!body) return;

  function byId(id) { return document.getElementById(id); }
  function attr(el, name) { return el && el.getAttribute ? el.getAttribute(name) : null; }
  function setText(el, text) {
    if (!el) return;
    if (typeof el.textContent !== 'undefined') el.textContent = text;
    else el.innerText = text;
  }
  function setStatus(text) {
    setText(byId('status'), text);
  }
  function addEvent(el, type, fn) {
    if (!el) return;
    if (el.addEventListener) el.addEventListener(type, fn, false);
    else if (el.attachEvent) el.attachEvent('on' + type, fn);
  }
  function each(list, fn) {
    if (!list) return;
    for (var i = 0; i < list.length; i++) fn(list[i]);
  }
  function eachTag(tag, fn) { each(document.getElementsByTagName(tag), fn); }
  function hasClass(el, name) {
    return !!(el && el.className && (' ' + el.className + ' ').indexOf(' ' + name + ' ') !== -1);
  }
  function addClass(el, name) {
    if (el && !hasClass(el, name)) el.className = (el.className ? el.className + ' ' : '') + name;
  }
  function removeClass(el, name) {
    if (!el || !el.className) return;
    el.className = (' ' + el.className + ' ').replace(' ' + name + ' ', ' ').replace(/^\s+|\s+$/g, '');
  }
  function reveal(el, display) {
    if (!el) return;
    removeClass(el, 'js-only');
    el.style.display = display || '';
  }
  function createXhr() {
    if (window.XMLHttpRequest) {
      try { return new XMLHttpRequest(); } catch (e) {}
    }
    if (window.ActiveXObject) {
      try { return new ActiveXObject('Microsoft.XMLHTTP'); } catch (e) {}
    }
    return null;
  }

  var path = attr(body, 'data-path') || window.location.pathname;
  var canUpload = attr(body, 'data-upload') === '1';

  function putFile(file, done, fail) {
    var xhr = createXhr();
    if (!xhr) { fail('XMLHttpRequest is not available'); return; }
    try { xhr.open('PUT', path + encodeURIComponent(file.name)); }
    catch (e) { fail('Cannot start upload'); return; }
    if (xhr.upload && xhr.upload.onprogress) {
      xhr.upload.onprogress = function (event) {
        if (event.lengthComputable) {
          setStatus(file.name + ': ' + Math.round(event.loaded * 100 / event.total) + '%');
        }
      };
    }
    xhr.onreadystatechange = function () {
      if (xhr.readyState !== 4) return;
      if (xhr.status >= 200 && xhr.status < 300) { done(); return; }
      var text = '';
      try { text = xhr.responseText || ''; } catch (e) {}
      fail(file.name + ': ' + xhr.status + ' ' + text);
    };
    try { xhr.send(file); }
    catch (e) { fail(file.name + ': cannot send file'); }
  }

  function uploadAll(files) {
    if (!files || !files.length) return;
    var index = 0;
    function next() {
      if (index >= files.length) { location.reload(); return; }
      var file = files[index++];
      setStatus('Uploading ' + file.name);
      putFile(file, next, function (message) { setStatus(message); });
    }
    next();
  }

  if (canUpload) {
    var input = byId('file');
    var probe = createXhr();
    var fileApi = !!(input && input.files && probe && probe.upload);
    if (fileApi) {
      reveal(byId('js-tools'), '');
      if (input) addEvent(input, 'change', function () { uploadAll(input.files); });

      var mkdir = byId('mkdir');
      if (mkdir) addEvent(mkdir, 'click', function () {
        var name = prompt('Folder name');
        if (!name) return;
        var xhr = createXhr();
        if (!xhr) { setStatus('XMLHttpRequest is not available'); return; }
        try { xhr.open('PUT', path + encodeURIComponent(name) + '/'); }
        catch (e) { setStatus('Cannot create folder'); return; }
        xhr.onreadystatechange = function () {
          if (xhr.readyState !== 4) return;
          if (xhr.status >= 200 && xhr.status < 300) { location.reload(); return; }
          var text = '';
          try { text = xhr.responseText || ''; } catch (e) {}
          setStatus(xhr.status + ' ' + text);
        };
        xhr.send(null);
      });

      var drop = byId('drop');
      if (drop) {
        reveal(drop, 'block');
        addEvent(document, 'dragenter', function (event) {
          if (event.preventDefault) event.preventDefault();
          addClass(drop, 'over');
        });
        addEvent(document, 'dragover', function (event) {
          if (event.preventDefault) event.preventDefault();
          addClass(drop, 'over');
        });
        addEvent(document, 'dragleave', function (event) {
          if (event.preventDefault) event.preventDefault();
          removeClass(drop, 'over');
        });
        addEvent(document, 'drop', function (event) {
          if (event.preventDefault) event.preventDefault();
          removeClass(drop, 'over');
          var transfer = event.dataTransfer;
          if (transfer) uploadAll(transfer.files);
        });
      }

      var classic = byId('classic-upload');
      if (classic) classic.style.display = 'none';
    }
  }

  eachTag('time', function (node) {
    var stamp = attr(node, 'data-ts');
    if (stamp === null || stamp === '') return;
    var date = new Date(Number(stamp) * 1000);
    if (!isNaN(date.getTime())) setText(node, date.toLocaleString());
  });

  var direction = 1;
  function rowData(row, key) {
    var value = attr(row, 'data-' + key);
    return value === null ? '' : value;
  }
  function sortBy(key) {
    var tbody = byId('rows');
    if (!tbody) return;
    var rows = [];
    var all = tbody.getElementsByTagName('tr');
    for (var i = 0; i < all.length; i++) {
      if (!hasClass(all[i], 'parent') && !hasClass(all[i], 'empty')) rows.push(all[i]);
    }
    rows.sort(function (a, b) {
      var left = rowData(a, key);
      var right = rowData(b, key);
      if (key === 'size' || key === 'ts') {
        return (Number(left) - Number(right)) * direction;
      }
      left = left.toLowerCase();
      right = right.toLowerCase();
      if (left < right) return -direction;
      if (left > right) return direction;
      return 0;
    });
    for (var j = 0; j < rows.length; j++) tbody.appendChild(rows[j]);
    direction = -direction;
  }
  eachTag('button', function (button) {
    var key = attr(button, 'data-sort');
    if (!key) return;
    addEvent(button, 'click', function () { sortBy(key); });
  });
})();
"###;

/// Render a directory listing into `out`.
///
/// `url_path` must be a normalized path that ends in `/`. Returns `false` only
/// when the fixed buffer cannot even hold the page chrome.
pub fn render<P: Platform>(
    platform: &mut P,
    dir: &mut P::Dir,
    url_path: &[u8],
    upload: bool,
    out: &mut FixedBuf<LISTING_BUF>,
) -> bool {
    if !url_path.starts_with(b"/") || !url_path.ends_with(b"/") {
        return false;
    }

    let mut encoded_path = FixedBuf::<MAX_PATH>::new();
    if !url_encode_path(&mut encoded_path, url_path) {
        return false;
    }

    if !out.push(b"<!doctype html>\n<html lang=\"en\">\n<head>\n") {
        return false;
    }
    if !out.push(b"<meta charset=\"utf-8\">\n")
        || !out.push(b"<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n")
        || !out.push(b"<meta name=\"color-scheme\" content=\"light dark\">\n")
        || !out.push(b"<meta name=\"referrer\" content=\"no-referrer\">\n")
        || !out.push(b"<title>Index of ")
        || !escape_html(out, url_path)
        || !out.push(b"</title>\n<style>")
        || !out.push(CSS)
        || !out.push(b"</style>\n</head>\n<body data-path=\"")
        || !out.push(encoded_path.as_slice())
        || !out.push(b"\" data-upload=\"")
        || !out.push(if upload { b"1" } else { b"0" })
        || !out.push(b"\">\n<header><a class=\"brand\" href=\"/\">sysweb</a>")
        || !out.push(b"<nav class=\"crumb\" aria-label=\"breadcrumb\">")
        || !render_crumb(out, url_path)
        || !out.push(b"</nav></header>\n<main>\n<h1>Index of <span class=\"path\">")
        || !escape_html(out, url_path)
        || !out.push(b"</span></h1>\n")
    {
        return false;
    }

    if upload {
        if !out.push(b"<form id=\"classic-upload\" class=\"tools\" method=\"post\" enctype=\"multipart/form-data\" action=\"\">")
            || !out.push(b"<label class=\"btn primary\">Upload (form)<input type=\"file\" name=\"file\"></label>")
            || !out.push(b"<button class=\"btn\" type=\"submit\">Upload</button>")
            || !out.push(b"</form>\n")
            || !out.push(b"<div id=\"js-tools\" class=\"tools js-only\">")
            || !out.push(
                b"<label class=\"btn primary\">Upload<input id=\"file\" type=\"file\" multiple></label>",
            )
            || !out.push(
                b"<button id=\"mkdir\" class=\"btn\" type=\"button\">New folder</button>",
            )
            || !out.push(b"<span id=\"status\" role=\"status\" aria-live=\"polite\"></span>")
            || !out.push(b"</div>\n<div id=\"drop\" class=\"js-only\">Drop files to upload</div>\n")
        {
            return false;
        }
    } else if !out.push(
        b"<p class=\"note\">Read-only. Start with <code>--upload</code> to enable uploads.</p>\n",
    ) {
        return false;
    }

    if !out.push(b"<table>\n<thead><tr>")
        || !out.push(b"<th><button data-sort=\"name\">Name</button></th>")
        || !out.push(b"<th><button data-sort=\"ts\">Modified</button></th>")
        || !out.push(b"<th><button data-sort=\"size\">Size</button></th>")
        || !out.push(b"<th><button data-sort=\"type\">Type</button></th>")
        || !out.push(b"</tr></thead>\n<tbody id=\"rows\">\n")
    {
        return false;
    }

    if url_path != b"/" && !render_parent(out, url_path) {
        return false;
    }

    let mut name = [0u8; 256];
    let mut entries = 0usize;
    let mut truncated = false;
    loop {
        if out.remaining() < 16_384 {
            truncated = true;
            break;
        }
        match platform.next_dir(dir, &mut name) {
            Ok(Some(entry)) => {
                if !render_entry(out, encoded_path.as_slice(), &name[..entry.name_len], entry) {
                    return false;
                }
                entries += 1;
            }
            Ok(None) => break,
            Err(_) => return false,
        }
    }

    if entries == 0
        && !out.push(b"<tr class=\"empty\"><td colspan=\"4\">This directory is empty.</td></tr>\n")
    {
        return false;
    }
    if truncated
        && !out.push(
            b"<tr class=\"empty\"><td colspan=\"4\">Listing truncated; open a subdirectory or use the shell.</td></tr>\n",
        )
    {
        return false;
    }

    if !out.push(b"</tbody></table>\n</main>\n<footer>")
        || !out.push(b"sysweb ")
        || !out.push(crate::VERSION.as_bytes())
        || !out.push(b" &middot; no_std &middot; no alloc &middot; syscalls")
        || !out.push(b"</footer>\n<details><summary>Terminal transfer</summary><pre>")
        || !out.push(b"# upload (when --upload is enabled)\ncurl -T FILE URL\n\n")
        || !out.push(b"# download\ncurl -O URL</pre></details>\n<script>")
        || !out.push(JS)
        || !out.push(b"</script>\n</body>\n</html>\n")
    {
        return false;
    }

    true
}

fn render_crumb<const N: usize>(out: &mut FixedBuf<N>, url_path: &[u8]) -> bool {
    if url_path == b"/" {
        return out.push(b"<span class=\"here\">/</span>");
    }
    if !out.push(b"<a href=\"/\">/</a><span class=\"sep\">/</span>") {
        return false;
    }
    let mut href = FixedBuf::<MAX_PATH>::new();
    if !href.push(b"/") {
        return false;
    }
    for segment in url_path[1..].split(|&byte| byte == b'/') {
        if segment.is_empty() {
            continue;
        }
        if !url_encode_segment(&mut href, segment)
            || !href.push_byte(b'/')
            || !out.push(b"<a href=\"")
            || !out.push(href.as_slice())
            || !out.push(b"\">")
            || !escape_html(out, segment)
            || !out.push(b"</a><span class=\"sep\">/</span>")
        {
            return false;
        }
    }
    true
}

fn render_parent<const N: usize>(out: &mut FixedBuf<N>, url_path: &[u8]) -> bool {
    let mut parent = FixedBuf::<MAX_PATH>::new();
    if !parent_path(url_path, &mut parent) {
        return false;
    }
    if !out.push(
        b"<tr class=\"parent\" data-name=\"..\" data-ts=\"0\" data-size=\"0\" data-type=\"dir\">",
    ) || !out.push(b"<td class=\"name\"><a href=\"")
        || !out.push(parent.as_slice())
        || !out.push(b"\">../</a></td><td class=\"modified\">&mdash;</td>")
        || !out.push(b"<td class=\"size\">&mdash;</td><td class=\"type\">dir</td></tr>\n")
    {
        return false;
    }
    true
}

fn parent_path(url_path: &[u8], out: &mut FixedBuf<MAX_PATH>) -> bool {
    let without_trailing = &url_path[..url_path.len() - 1];
    let slash = without_trailing
        .iter()
        .rposition(|&byte| byte == b'/')
        .unwrap_or(0);
    url_encode_path(out, &url_path[..slash + 1])
}

fn render_entry<const N: usize>(
    out: &mut FixedBuf<N>,
    encoded_path: &[u8],
    name: &[u8],
    entry: DirEntry,
) -> bool {
    let is_dir = entry.kind == EntryKind::Dir;
    let is_link = entry.kind == EntryKind::Symlink;

    let mut href = FixedBuf::<MAX_PATH>::new();
    if !href.push(encoded_path) || !url_encode_segment(&mut href, name) {
        return false;
    }
    if is_dir && !href.push_byte(b'/') {
        return false;
    }

    let data_type: &[u8] = match entry.kind {
        EntryKind::Dir => b"dir",
        EntryKind::Symlink => b"link",
        EntryKind::File => b"file",
        EntryKind::Other => b"other",
    };
    let display_type: &'static [u8] = match entry.kind {
        EntryKind::Dir => b"dir",
        EntryKind::Symlink => b"link",
        EntryKind::File => file_type(name),
        EntryKind::Other => b"other",
    };

    let mut size_buf = [0u8; 32];
    let size: &[u8] = if entry.kind == EntryKind::File {
        let len = format_size(entry.len, &mut size_buf);
        &size_buf[..len]
    } else {
        b"\xe2\x80\x94" // em dash
    };

    let mut time_buf = [0u8; 20];
    let time_len = format_time(entry.mtime, &mut time_buf);

    if !out.push(b"<tr data-name=\"")
        || !escape_html(out, name)
        || !out.push(b"\" data-ts=\"")
        || !out.push_u64(entry.mtime.max(0) as u64)
        || !out.push(b"\" data-size=\"")
        || !out.push_u64(entry.len)
        || !out.push(b"\" data-type=\"")
        || !out.push(data_type)
        || !out.push(b"\"><td class=\"name\">")
    {
        return false;
    }

    if is_link {
        if !out.push(b"<span class=\"symlink\" title=\"symlinks are not served\">")
            || !escape_html(out, name)
            || !out.push(b"</span>")
        {
            return false;
        }
    } else {
        if !out.push(b"<a href=\"")
            || !out.push(href.as_slice())
            || !out.push(b"\">")
            || !escape_html(out, name)
        {
            return false;
        }
        if is_dir && !out.push_byte(b'/') {
            return false;
        }
        if !out.push(b"</a>") {
            return false;
        }
    }

    if !out.push(b"</td><td class=\"modified\"><time data-ts=\"")
        || !out.push_u64(entry.mtime.max(0) as u64)
        || !out.push(b"\">")
        || !out.push(&time_buf[..time_len])
        || !out.push(b"</time></td><td class=\"size\">")
        || !out.push(size)
        || !out.push(b"</td><td class=\"type\">")
        || !out.push(display_type)
        || !out.push(b"</td></tr>\n")
    {
        return false;
    }
    true
}

fn file_type(name: &[u8]) -> &'static [u8] {
    let mime = mime::content_type(name);
    let end = mime
        .iter()
        .position(|&byte| byte == b';')
        .unwrap_or(mime.len());
    let mut essence = &mime[..end];
    while let [rest @ .., b' '] = essence {
        essence = rest;
    }
    if essence == b"application/octet-stream" {
        b"file"
    } else {
        essence
    }
}

fn escape_html<const N: usize>(out: &mut FixedBuf<N>, bytes: &[u8]) -> bool {
    for &byte in bytes {
        let replacement: &[u8] = match byte {
            b'&' => b"&amp;",
            b'<' => b"&lt;",
            b'>' => b"&gt;",
            b'"' => b"&quot;",
            b'\'' => b"&#39;",
            _ => {
                if !out.push_byte(byte) {
                    return false;
                }
                continue;
            }
        };
        if !out.push(replacement) {
            return false;
        }
    }
    true
}

pub(crate) fn url_encode_path<const N: usize>(out: &mut FixedBuf<N>, path: &[u8]) -> bool {
    url_encode(out, path, true)
}

fn url_encode_segment<const N: usize>(out: &mut FixedBuf<N>, segment: &[u8]) -> bool {
    url_encode(out, segment, false)
}

fn url_encode<const N: usize>(out: &mut FixedBuf<N>, bytes: &[u8], keep_slash: bool) -> bool {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for &byte in bytes {
        let unreserved = byte.is_ascii_alphanumeric()
            || byte == b'-'
            || byte == b'.'
            || byte == b'_'
            || byte == b'~'
            || (keep_slash && byte == b'/');
        if unreserved {
            if !out.push_byte(byte) {
                return false;
            }
        } else if !out.push_byte(b'%')
            || !out.push_byte(HEX[(byte >> 4) as usize])
            || !out.push_byte(HEX[(byte & 0x0f) as usize])
        {
            return false;
        }
    }
    true
}

fn format_size(value: u64, out: &mut [u8; 32]) -> usize {
    const UNITS: [&[u8]; 6] = [b"kB", b"MB", b"GB", b"TB", b"PB", b"EB"];
    let mut len = 0usize;

    if value < 1000 {
        len += write_u64(value, &mut out[len..]);
        out[len..len + 2].copy_from_slice(b" B");
        return len + 2;
    }

    let mut divisor = 1000u64;
    let mut unit = 0usize;
    while unit + 1 < UNITS.len() && value / divisor >= 1000 {
        divisor *= 1000;
        unit += 1;
    }

    let whole = value / divisor;
    let fraction = (value % divisor) * 10 / divisor;
    len += write_u64(whole, &mut out[len..]);
    if fraction != 0 {
        out[len] = b'.';
        out[len + 1] = b'0' + fraction as u8;
        len += 2;
    }
    out[len] = b' ';
    len += 1;
    out[len..len + UNITS[unit].len()].copy_from_slice(UNITS[unit]);
    len + UNITS[unit].len()
}

fn write_u64(mut value: u64, out: &mut [u8]) -> usize {
    let mut digits = [0u8; 20];
    let mut at = digits.len();
    loop {
        at -= 1;
        digits[at] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    let len = digits.len() - at;
    out[..len].copy_from_slice(&digits[at..]);
    len
}

/// Format a Unix timestamp as `YYYY-MM-DD HH:MM` (UTC). JavaScript upgrades it
/// to the viewer's local time when available.
fn format_time(timestamp: i64, out: &mut [u8; 20]) -> usize {
    // Clamp to 9999-12-31 so a hostile mtime cannot overflow the buffer.
    let seconds = timestamp.clamp(0, 253_402_300_799) as u64;
    let days = (seconds / 86_400) as i64;
    let rem = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    let hour = rem / 3_600;
    let minute = (rem % 3_600) / 60;

    let mut len = 0usize;
    len += write_padded(year.max(0) as u64, 4, &mut out[len..]);
    out[len] = b'-';
    len += 1;
    len += write_padded(month as u64, 2, &mut out[len..]);
    out[len] = b'-';
    len += 1;
    len += write_padded(day as u64, 2, &mut out[len..]);
    out[len] = b' ';
    len += 1;
    len += write_padded(hour, 2, &mut out[len..]);
    out[len] = b':';
    len += 1;
    len += write_padded(minute, 2, &mut out[len..]);
    len
}

fn write_padded(mut value: u64, width: usize, out: &mut [u8]) -> usize {
    let mut digits = [b'0'; 20];
    let mut at = digits.len();
    loop {
        at -= 1;
        digits[at] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    let produced = digits.len() - at;
    let width = width.max(produced);
    let padding = width - produced;
    for slot in out[..padding].iter_mut() {
        *slot = b'0';
    }
    out[padding..width].copy_from_slice(&digits[at..]);
    width
}

/// Howard Hinnant's civil-from-days algorithm.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    (year + i64::from(month <= 2), month, day)
}

#[cfg(test)]
mod tests {
    use super::{civil_from_days, format_size, format_time};

    #[test]
    fn sizes_are_si() {
        let mut buf = [0u8; 32];
        let cases: &[(u64, &[u8])] = &[
            (0, b"0 B"),
            (999, b"999 B"),
            (1000, b"1 kB"),
            (1500, b"1.5 kB"),
            (1_000_000, b"1 MB"),
            (1_234_567, b"1.2 MB"),
            (2_000_000_000, b"2 GB"),
        ];
        for (value, expected) in cases {
            let len = format_size(*value, &mut buf);
            assert_eq!(&buf[..len], *expected, "size {value}");
        }
    }

    #[test]
    fn times_are_utc() {
        let mut buf = [0u8; 20];
        let cases: &[(i64, &[u8])] = &[
            (0, b"1970-01-01 00:00"),
            (951_782_400, b"2000-02-29 00:00"),
            (1_700_000_000, b"2023-11-14 22:13"),
        ];
        for (timestamp, expected) in cases {
            let len = format_time(*timestamp, &mut buf);
            assert_eq!(&buf[..len], *expected, "time {timestamp}");
        }
    }

    #[test]
    fn civil_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(10_957), (2000, 1, 1));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
    }
}
