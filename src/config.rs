//! Command-line configuration. No heap: the root path lives in a fixed array.

use crate::platform::SocketAddr;

pub const DEFAULT_PORT: u16 = 8080;
pub const DEFAULT_BACKLOG: u16 = 128;
pub const MAX_ROOT: usize = 256;

pub const USAGE: &str = concat!(
    "syswebd ",
    env!("CARGO_PKG_VERSION"),
    " - a no_std, no-alloc static web server\n\
     \n\
     USAGE:\n\
     \x20   syswebd [OPTIONS]\n\
     \n\
     OPTIONS:\n\
     \x20   -r, --root <DIR>       Directory to serve (default: .)\n\
     \x20   -p, --port <PORT>      TCP port, 1-65535 (default: 8080)\n\
     \x20   -H, --host <ADDR>      IPv4 address to bind (default: 127.0.0.1)\n\
     \x20       --backlog <N>      Listen backlog (default: 128)\n\
     \x20       --upload           Enable PUT uploads and folder creation\n\
     \x20       --replace          Allow uploads to replace existing files\n\
     \x20       --no-index         Disable generated directory indexes\n\
     \x20   -q, --quiet            Suppress startup and request logs\n\
     \x20   -h, --help             Print this help\n\
     \x20   -V, --version          Print the version\n"
);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigError {
    MissingValue,
    InvalidPort,
    InvalidHost,
    InvalidBacklog,
    RootEmpty,
    RootTooLong,
    EmbeddedNul,
    UnknownOption,
}

impl ConfigError {
    pub const fn message(self) -> &'static [u8] {
        match self {
            ConfigError::MissingValue => b"option requires a value",
            ConfigError::InvalidPort => b"port must be an integer in 1..=65535",
            ConfigError::InvalidHost => b"host must be a dotted-quad IPv4 address",
            ConfigError::InvalidBacklog => b"backlog must be an integer in 1..=65535",
            ConfigError::RootEmpty => b"root directory must not be empty",
            ConfigError::RootTooLong => b"root directory is too long",
            ConfigError::EmbeddedNul => b"root directory contains a NUL byte",
            ConfigError::UnknownOption => b"unknown option (try --help)",
        }
    }
}

#[derive(Debug)]
pub struct Config {
    root: [u8; MAX_ROOT],
    root_len: usize,
    addr: SocketAddr,
    backlog: u16,
    quiet: bool,
    upload: bool,
    replace: bool,
    index: bool,
    port_explicit: bool,
}

#[derive(Debug)]
// The config is intentionally stored inline: this crate has no allocator, so
// boxing the large variant is not an option.
#[allow(clippy::large_enum_variant)]
pub enum ParseOutcome {
    Run(Config),
    Help,
    Version,
}

impl Config {
    pub fn parse(args: &[&[u8]]) -> Result<ParseOutcome, ConfigError> {
        let mut port = DEFAULT_PORT;
        let mut host = [127, 0, 0, 1];
        let mut backlog = DEFAULT_BACKLOG;
        let mut root: &[u8] = b".";
        let mut quiet = false;
        let mut upload = false;
        let mut replace = false;
        let mut index = true;
        let mut port_explicit = false;

        let mut i = 1usize; // skip argv[0]
        while i < args.len() {
            let arg = args[i];

            if arg == b"--help" || arg == b"-h" {
                return Ok(ParseOutcome::Help);
            }
            if arg == b"--version" || arg == b"-V" {
                return Ok(ParseOutcome::Version);
            }
            if arg == b"--quiet" || arg == b"-q" {
                quiet = true;
                i += 1;
                continue;
            }
            if arg == b"--upload" {
                upload = true;
                i += 1;
                continue;
            }
            if arg == b"--replace" {
                replace = true;
                i += 1;
                continue;
            }
            if arg == b"--no-index" {
                index = false;
                i += 1;
                continue;
            }
            if arg == b"--index" {
                index = true;
                i += 1;
                continue;
            }

            if let Some(value) = arg.strip_prefix(b"--port=") {
                port = parse_port(value)?;
                port_explicit = true;
                i += 1;
                continue;
            }
            if let Some(value) = arg.strip_prefix(b"--root=") {
                root = value;
                i += 1;
                continue;
            }
            if let Some(value) = arg.strip_prefix(b"--host=") {
                host = parse_ipv4(value)?;
                i += 1;
                continue;
            }
            if let Some(value) = arg.strip_prefix(b"--backlog=") {
                backlog = parse_backlog(value)?;
                i += 1;
                continue;
            }

            if arg == b"--port" || arg == b"-p" {
                let value = take(args, &mut i)?;
                port = parse_port(value)?;
                port_explicit = true;
                i += 1;
                continue;
            }
            if arg == b"--root" || arg == b"-r" {
                let value = take(args, &mut i)?;
                root = value;
                i += 1;
                continue;
            }
            if arg == b"--host" || arg == b"-H" {
                let value = take(args, &mut i)?;
                host = parse_ipv4(value)?;
                i += 1;
                continue;
            }
            if arg == b"--backlog" {
                let value = take(args, &mut i)?;
                backlog = parse_backlog(value)?;
                i += 1;
                continue;
            }

            return Err(ConfigError::UnknownOption);
        }

        let mut root_buf = [0u8; MAX_ROOT];
        let root_len = store_root(root, &mut root_buf)?;

        Ok(ParseOutcome::Run(Config {
            root: root_buf,
            root_len,
            addr: SocketAddr::new(host, port),
            backlog,
            quiet,
            upload,
            replace,
            index,
            port_explicit,
        }))
    }

    pub fn root(&self) -> &[u8] {
        &self.root[..self.root_len]
    }

    pub const fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub const fn backlog(&self) -> u16 {
        self.backlog
    }

    pub const fn quiet(&self) -> bool {
        self.quiet
    }

    pub const fn upload(&self) -> bool {
        self.upload
    }

    pub const fn replace(&self) -> bool {
        self.replace
    }

    pub const fn index(&self) -> bool {
        self.index
    }

    /// True when the port came from `--port`; the implicit default may fall
    /// back to the next free port.
    pub const fn port_explicit(&self) -> bool {
        self.port_explicit
    }
}

fn take<'a>(args: &'a [&'a [u8]], i: &mut usize) -> Result<&'a [u8], ConfigError> {
    *i += 1;
    args.get(*i).copied().ok_or(ConfigError::MissingValue)
}

fn parse_port(value: &[u8]) -> Result<u16, ConfigError> {
    let parsed = parse_u16(value).ok_or(ConfigError::InvalidPort)?;
    if parsed == 0 {
        return Err(ConfigError::InvalidPort);
    }
    Ok(parsed)
}

fn parse_backlog(value: &[u8]) -> Result<u16, ConfigError> {
    let parsed = parse_u16(value).ok_or(ConfigError::InvalidBacklog)?;
    if parsed == 0 {
        return Err(ConfigError::InvalidBacklog);
    }
    Ok(parsed)
}

fn parse_u16(value: &[u8]) -> Option<u16> {
    if value.is_empty() {
        return None;
    }
    let mut number: u32 = 0;
    for &byte in value {
        if !byte.is_ascii_digit() {
            return None;
        }
        number = number * 10 + (byte - b'0') as u32;
        if number > u16::MAX as u32 {
            return None;
        }
    }
    Some(number as u16)
}

fn parse_ipv4(value: &[u8]) -> Result<[u8; 4], ConfigError> {
    let mut octets = [0u8; 4];
    let mut index = 0usize;
    let mut digits = 0u32;
    let mut number = 0u32;

    for &byte in value {
        if byte == b'.' {
            if digits == 0 || index == 3 {
                return Err(ConfigError::InvalidHost);
            }
            octets[index] = number as u8;
            index += 1;
            digits = 0;
            number = 0;
        } else if byte.is_ascii_digit() {
            number = number * 10 + (byte - b'0') as u32;
            digits += 1;
            if number > 255 || digits > 3 {
                return Err(ConfigError::InvalidHost);
            }
        } else {
            return Err(ConfigError::InvalidHost);
        }
    }

    if index != 3 || digits == 0 {
        return Err(ConfigError::InvalidHost);
    }
    octets[3] = number as u8;
    Ok(octets)
}

fn store_root(root: &[u8], out: &mut [u8; MAX_ROOT]) -> Result<usize, ConfigError> {
    if root.is_empty() {
        return Err(ConfigError::RootEmpty);
    }
    if root.contains(&0) {
        return Err(ConfigError::EmbeddedNul);
    }

    let mut end = root.len();
    while end > 1 && root[end - 1] == b'/' {
        end -= 1;
    }
    if end > MAX_ROOT {
        return Err(ConfigError::RootTooLong);
    }
    out[..end].copy_from_slice(&root[..end]);
    Ok(end)
}

#[cfg(test)]
mod tests {
    use super::{Config, ConfigError, ParseOutcome, MAX_ROOT};

    fn run(args: &[&[u8]]) -> Config {
        match Config::parse(args).expect("parse") {
            ParseOutcome::Run(config) => config,
            other => panic!("expected Run, got {other:?}"),
        }
    }

    #[test]
    fn defaults() {
        let config = run(&[b"syswebd"]);
        assert_eq!(config.root(), b".");
        assert_eq!(config.addr().ip, [127, 0, 0, 1]);
        assert_eq!(config.addr().port, 8080);
        assert_eq!(config.backlog(), 128);
        assert!(!config.quiet());
        assert!(!config.upload());
        assert!(!config.replace());
        assert!(config.index());
        assert!(!config.port_explicit());
    }

    #[test]
    fn parses_options() {
        let config = run(&[
            b"syswebd",
            b"--port",
            b"9000",
            b"-r",
            b"/srv/www",
            b"--host=0.0.0.0",
            b"--backlog",
            b"64",
            b"-q",
            b"--upload",
            b"--replace",
            b"--no-index",
        ]);
        assert_eq!(config.root(), b"/srv/www");
        assert_eq!(config.addr().ip, [0, 0, 0, 0]);
        assert_eq!(config.addr().port, 9000);
        assert_eq!(config.backlog(), 64);
        assert!(config.quiet());
        assert!(config.upload());
        assert!(config.replace());
        assert!(!config.index());
        assert!(config.port_explicit());
    }

    #[test]
    fn trims_tree_trailing_slashes_but_keeps_root() {
        assert_eq!(
            run(&[b"syswebd", b"--root", b"public///"]).root(),
            b"public"
        );
        assert_eq!(run(&[b"syswebd", b"--root", b"///"]).root(), b"/");
        assert_eq!(run(&[b"syswebd", b"--root", b"/tmp/"]).root(), b"/tmp");
    }

    #[test]
    fn rejects_bad_values() {
        for args in [
            &[b"syswebd".as_slice(), b"--port", b"0"][..],
            &[b"syswebd".as_slice(), b"--port", b"65536"][..],
            &[b"syswebd".as_slice(), b"--port", b"abc"][..],
            &[b"syswebd".as_slice(), b"--port"][..],
            &[b"syswebd".as_slice(), b"--host", b"1.2.3"][..],
            &[b"syswebd".as_slice(), b"--host", b"1.2.3.256"][..],
            &[b"syswebd".as_slice(), b"--backlog", b"0"][..],
            &[b"syswebd".as_slice(), b"--nope"][..],
        ] {
            assert!(Config::parse(args).is_err(), "accepted {args:?}");
        }
    }

    #[test]
    fn help_and_version() {
        assert!(matches!(
            Config::parse(&[b"syswebd", b"--help"]).unwrap(),
            ParseOutcome::Help
        ));
        assert!(matches!(
            Config::parse(&[b"syswebd", b"-V"]).unwrap(),
            ParseOutcome::Version
        ));
    }

    #[test]
    fn root_limits() {
        let mut long = [b'a'; MAX_ROOT + 1];
        long[MAX_ROOT] = b'x';
        assert_eq!(
            Config::parse(&[b"syswebd", b"--root", &long]).unwrap_err(),
            ConfigError::RootTooLong
        );
    }
}
