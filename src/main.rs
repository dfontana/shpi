mod client;
mod daemon;
mod error;
mod frame;
mod paths;
mod pidfile;
mod protocol;
mod state;

use argh::FromArgs;
use std::path::PathBuf;

/// shpi — pipe string data from remote Linux hosts to a Mac listener over SSH
/// reverse tunnels.
#[derive(FromArgs)]
struct Cli {
    #[argh(subcommand)]
    cmd: Cmd,
}

#[derive(FromArgs)]
#[argh(subcommand)]
enum Cmd {
    Start(StartCmd),
    Stop(StopCmd),
    Status(StatusCmd),
    Watch(WatchCmd),
    Send(SendCmd),
}

/// Start the receiver daemon and SSH tunnels (Mac side).
#[derive(FromArgs)]
#[argh(subcommand, name = "start")]
struct StartCmd {
    /// TCP port for the listener and remote forwarded port (default 9292)
    #[argh(option, default = "9292")]
    port: u16,
    /// daemon log file path
    #[argh(option)]
    log: Option<PathBuf>,
    /// one or more user@host targets
    #[argh(positional)]
    hosts: Vec<String>,
}

/// Stop the running daemon.
#[derive(FromArgs)]
#[argh(subcommand, name = "stop")]
struct StopCmd {}

/// Show per-host tunnel status.
#[derive(FromArgs)]
#[argh(subcommand, name = "status")]
struct StatusCmd {}

/// Stream received messages to stdout.
#[derive(FromArgs)]
#[argh(subcommand, name = "watch")]
struct WatchCmd {
    /// render each message as an OSC-99 notification escape sequence
    #[argh(switch)]
    osc99: bool,
}

/// Send a message to the receiver (remote Linux side).
#[derive(FromArgs)]
#[argh(subcommand, name = "send")]
struct SendCmd {
    /// port to connect to (default 9292)
    #[argh(option, default = "9292")]
    port: u16,
    /// hostname to attach (default: system hostname)
    #[argh(option)]
    name: Option<String>,
    /// the message to send
    #[argh(positional)]
    message: String,
}

fn main() {
    let cli: Cli = argh::from_env();
    let result = match cli.cmd {
        Cmd::Start(c) => {
            if c.hosts.is_empty() {
                Err(error::Error::msg("at least one user@host is required"))
            } else {
                daemon::run_start(c.port, c.log, c.hosts)
            }
        }
        Cmd::Stop(_) => client::stop(),
        Cmd::Status(_) => client::status(),
        Cmd::Watch(c) => client::watch(c.osc99),
        Cmd::Send(c) => client::send(c.port, c.name, c.message),
    };
    if let Err(e) = result {
        eprintln!("shpi: {e}");
        std::process::exit(1);
    }
}
