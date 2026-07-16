#![forbid(unsafe_code)]

use std::{
    env,
    net::SocketAddr,
    path::PathBuf,
    process::ExitCode,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use pcloud_secret::secret_string::SecretString;
use pcloud_web::{DEFAULT_BIND_ADDR, WebConfig, generate_web_token, serve};

const HELP: &str = "\
pcloud-web - pcloud-rs Web UI

USAGE:
    pcloud-web [OPTIONS]

OPTIONS:
    --bind <ADDR>             Address:port to bind
                              [default: 127.0.0.1:17650]
    --socket <PATH>           Daemon IPC socket path
                              [default: XDG runtime pcloud.sock]
    --web-token <TOKEN>       Use an explicit web session token
    --token <TOKEN>           Alias for --web-token
    --web-token-file <PATH>   Read the web session token from a file
    --token-file <PATH>       Alias for --web-token-file
    --allow-host <HOST>       Additional accepted Host value; repeatable.
                              Required for LAN/all-interface testing when
                              browsers use a non-local Host header.
    --ready                   Mark /readyz ready immediately [default]
    --not-ready               Leave /readyz at 503 until embedded code flips it
    -h, --help                Print help
    -V, --version             Print version
";

#[derive(Debug)]
struct Cli {
    bind_addr: SocketAddr,
    socket_path: Option<PathBuf>,
    token_source: TokenSource,
    allowed_hosts: Vec<String>,
    ready: bool,
    mode: Mode,
}

#[derive(Debug, Default)]
enum TokenSource {
    #[default]
    Generate,
    Literal(String),
    File(PathBuf),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Serve,
    Help,
    Version,
}

impl Default for Cli {
    fn default() -> Self {
        Self {
            bind_addr: DEFAULT_BIND_ADDR,
            socket_path: None,
            token_source: TokenSource::Generate,
            allowed_hosts: Vec::new(),
            ready: true,
            mode: Mode::Serve,
        }
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("pcloud-web: {err}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), String> {
    let cli = Cli::parse(env::args().skip(1))?;
    match cli.mode {
        Mode::Help => {
            print!("{HELP}");
            return Ok(());
        }
        Mode::Version => {
            println!("pcloud-web {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Mode::Serve => {}
    }

    let socket_path = match cli.socket_path {
        Some(path) => path,
        None => default_socket_path()?,
    };
    let web_token = resolve_token(cli.token_source)?;
    let ready = Arc::new(AtomicBool::new(false));
    ready.store(cli.ready, Ordering::Release);

    let config = WebConfig {
        socket_path,
        bind_addr: cli.bind_addr,
        web_token,
        allowed_hosts: cli.allowed_hosts,
        ready,
    };
    serve(config).await.map_err(|err| err.to_string())
}

impl Cli {
    fn parse(args: impl IntoIterator<Item = String>) -> Result<Self, String> {
        let mut cli = Self::default();
        let mut args = args.into_iter();

        while let Some(arg) = args.next() {
            if arg == "--help" || arg == "-h" {
                cli.mode = Mode::Help;
                continue;
            }
            if arg == "--version" || arg == "-V" {
                cli.mode = Mode::Version;
                continue;
            }
            if arg == "--ready" {
                cli.ready = true;
                continue;
            }
            if arg == "--not-ready" {
                cli.ready = false;
                continue;
            }

            if let Some(value) = arg.strip_prefix("--bind=") {
                cli.bind_addr = parse_bind(value)?;
                continue;
            }
            if let Some(value) = arg.strip_prefix("--socket=") {
                cli.socket_path = Some(PathBuf::from(value));
                continue;
            }
            if let Some(value) = arg.strip_prefix("--web-token=") {
                set_token_source(
                    &mut cli.token_source,
                    TokenSource::Literal(value.to_owned()),
                )?;
                continue;
            }
            if let Some(value) = arg.strip_prefix("--token=") {
                set_token_source(
                    &mut cli.token_source,
                    TokenSource::Literal(value.to_owned()),
                )?;
                continue;
            }
            if let Some(value) = arg.strip_prefix("--web-token-file=") {
                set_token_source(
                    &mut cli.token_source,
                    TokenSource::File(PathBuf::from(value)),
                )?;
                continue;
            }
            if let Some(value) = arg.strip_prefix("--token-file=") {
                set_token_source(
                    &mut cli.token_source,
                    TokenSource::File(PathBuf::from(value)),
                )?;
                continue;
            }
            if let Some(value) = arg.strip_prefix("--allow-host=") {
                push_allowed_host(&mut cli.allowed_hosts, value)?;
                continue;
            }

            match arg.as_str() {
                "--bind" => {
                    let value = next_value(&mut args, "--bind")?;
                    cli.bind_addr = parse_bind(&value)?;
                }
                "--socket" => {
                    let value = next_value(&mut args, "--socket")?;
                    cli.socket_path = Some(PathBuf::from(value));
                }
                "--web-token" | "--token" => {
                    let value = next_value(&mut args, &arg)?;
                    set_token_source(&mut cli.token_source, TokenSource::Literal(value))?;
                }
                "--web-token-file" | "--token-file" => {
                    let value = next_value(&mut args, &arg)?;
                    set_token_source(
                        &mut cli.token_source,
                        TokenSource::File(PathBuf::from(value)),
                    )?;
                }
                "--allow-host" => {
                    let value = next_value(&mut args, "--allow-host")?;
                    push_allowed_host(&mut cli.allowed_hosts, &value)?;
                }
                _ => return Err(format!("unknown argument `{arg}`; try --help")),
            }
        }

        Ok(cli)
    }
}

fn next_value(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    args.next()
        .filter(|value| !value.starts_with('-'))
        .ok_or_else(|| format!("{flag} requires a value"))
}

fn parse_bind(value: &str) -> Result<SocketAddr, String> {
    value
        .parse()
        .map_err(|err| format!("invalid --bind `{value}`: {err}"))
}

fn set_token_source(current: &mut TokenSource, next: TokenSource) -> Result<(), String> {
    if !matches!(current, TokenSource::Generate) {
        return Err("choose only one token source".to_string());
    }
    *current = next;
    Ok(())
}

fn push_allowed_host(allowed_hosts: &mut Vec<String>, value: &str) -> Result<(), String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("--allow-host requires a non-empty host".to_string());
    }
    allowed_hosts.push(value.to_owned());
    Ok(())
}

fn resolve_token(source: TokenSource) -> Result<SecretString, String> {
    let token = match source {
        TokenSource::Generate => generate_web_token()?,
        TokenSource::Literal(token) => token,
        TokenSource::File(path) => {
            let raw = std::fs::read_to_string(&path)
                .map_err(|err| format!("read token file {}: {err}", path.display()))?;
            raw.trim_end_matches(['\r', '\n']).to_owned()
        }
    };
    if token.is_empty() {
        return Err("web token must not be empty".to_string());
    }
    Ok(SecretString::new(token))
}

fn default_socket_path() -> Result<PathBuf, String> {
    if let Some(root) = env::var_os("PCLOUD_ROOT") {
        return Ok(PathBuf::from(root).join("runtime").join("pcloud.sock"));
    }

    if let Some(runtime) = non_empty_env_path("XDG_RUNTIME_DIR") {
        return Ok(runtime.join("pcloud").join("pcloud-rs").join("pcloud.sock"));
    }

    let cache_root = non_empty_env_path("XDG_CACHE_HOME")
        .or_else(|| non_empty_env_path("HOME").map(|home| home.join(".cache")))
        .ok_or_else(|| {
            "could not resolve default socket path; set --socket or PCLOUD_ROOT".to_string()
        })?;
    Ok(cache_root
        .join("pcloud")
        .join("pcloud-rs")
        .join("pcloud-rs-runtime")
        .join("pcloud.sock"))
}

fn non_empty_env_path(name: &str) -> Option<PathBuf> {
    env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}
