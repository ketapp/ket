//! `ket host`: the process that runs the app's terminals, and its phones.

use clap::Subcommand;
use ket_core::KetError;
use ket_core::command::{self, Category, Invocation, Registry};

/// Attaches this module's implementations to the registry, registering
/// first the commands that are the CLI's alone — see `registry`.
pub(super) fn register(registry: &mut Registry) -> Result<(), KetError> {
    // The host's commands are the CLI's alone: from inside the app, stopping
    // the process that runs its terminals is not a thing to offer.
    registry.register(command::Command::new(
        "host.status",
        "Show whether the ket host is running",
        Category::Application,
    ))?;

    registry.register(
        command::Command::new(
            "host.stop",
            "Stop the ket host, if nothing is running in it",
            Category::Application,
        )
        .arg(command::Arg::flag("force")),
    )?;

    registry.register(command::Command::new(
        "host.pair",
        "Print a one-time code for pairing a phone",
        Category::Application,
    ))?;

    registry.register(command::Command::new(
        "host.devices",
        "List the phones paired with this Mac",
        Category::Application,
    ))?;

    registry.register(
        command::Command::new(
            "host.revoke",
            "Stop accepting a phone, and disconnect it",
            Category::Application,
        )
        .arg(command::Arg::required("id", command::ArgKind::Text)),
    )?;

    registry.bind("host.devices", |_, _| {
        let devices = ket_core::host::devices()?;
        if devices.is_empty() {
            println!("no phones paired — `ket host pair` makes a code");
        }
        for device in devices {
            println!(
                "{}  {}{}",
                device.id,
                if device.name.is_empty() {
                    "(unnamed)"
                } else {
                    &device.name
                },
                if device.connected {
                    "  · connected"
                } else {
                    ""
                }
            );
        }
        Ok(())
    })?;

    registry.bind("host.revoke", |invocation, _| {
        let id = invocation.require_text("id")?;
        if ket_core::host::revoke(id)? {
            println!("revoked");
            Ok(())
        } else {
            Err(KetError::Conflict(format!("no phone {id} is paired")))
        }
    })?;

    registry.bind("host.pair", |_, _| {
        println!("{}", ket_core::host::pairing_code()?);
        Ok(())
    })?;

    registry.bind("host.status", |_, _| {
        match ket_core::host::status()? {
            Some(status) => println!(
                "running: {} terminal(s)\nbuild:   {}\nsocket:  {}",
                status.live,
                status.build,
                ket_core::host::socket_path()?.display()
            ),
            None => println!("not running"),
        }
        Ok(())
    })?;

    registry.bind("host.stop", |invocation, _| {
        let force = invocation.is_set("force");
        match ket_core::host::status()? {
            None => println!("not running"),
            Some(status) if status.live > 0 && !force => {
                return Err(KetError::Conflict(format!(
                    "the host has {} terminal(s) running; it stays until they end \
                     (--force ends them)",
                    status.live
                )));
            }
            Some(status) => {
                ket_core::host::stop(force)?;
                match status.live {
                    0 => println!("stopped"),
                    n => println!("stopped, ending {n} terminal(s)"),
                }
            }
        }
        Ok(())
    })?;

    Ok(())
}

impl HostCmd {
    /// Turns parsed argv into a registry invocation — see `Cmd::invocation`.
    pub(super) fn invocation(self) -> Invocation {
        match self {
            HostCmd::Status => Invocation::new("host.status"),
            HostCmd::Stop { force } => Invocation::new("host.stop").flag("force", force),
            HostCmd::Pair => Invocation::new("host.pair"),
            HostCmd::Devices => Invocation::new("host.devices"),
            HostCmd::Revoke { id } => Invocation::new("host.revoke").text("id", id),
        }
    }
}

#[derive(Subcommand, Debug)]
pub(super) enum HostCmd {
    /// Say whether a host is running for this data directory, and what in.
    Status,
    /// Ask the host to exit. It only does if no terminal is running in it.
    Stop {
        /// End every terminal running in it, then exit. Through the host's
        /// socket, so it works where signals to the host do not arrive.
        #[arg(long)]
        force: bool,
    },
    /// Print a one-time code for pairing a phone, good for ten minutes.
    Pair,
    /// List the phones paired with this Mac.
    Devices,
    /// Stop accepting a phone, and disconnect it if it is connected.
    Revoke {
        /// The phone's id, from `ket host devices`.
        id: String,
    },
}

/// Every way of reaching this module's commands from argv, for the
/// registry tests in `main.rs`.
#[cfg(test)]
pub(super) const ARGV: &[&[&str]] = &[
    &["ket", "host", "status"],
    &["ket", "host", "stop"],
    &["ket", "host", "stop", "--force"],
    &["ket", "host", "pair"],
    &["ket", "host", "devices"],
    &["ket", "host", "revoke", "phone-1"],
];
