pub mod control_panel;
pub mod desktop_panel;
pub mod layout_editor;

use std::fs;
use std::path::{Path, PathBuf};

use kmsync_core::local_ipc::{
    default_local_ipc_endpoint, LocalIpcClient, LocalIpcEndpoint, LocalIpcRequest, LocalIpcResponse,
};

pub fn run_with_args(args: impl Iterator<Item = String>) -> Result<(), String> {
    let args = Args::parse(args)?;
    match args.command {
        UiCommand::Status { endpoint } => print_status(&endpoint),
        UiCommand::Ping { endpoint } => print_ping(&endpoint),
        UiCommand::LayoutEditor {
            profile_path,
            output_path,
        } => print_layout_editor(&profile_path, output_path.as_deref()),
        UiCommand::ControlPanel {
            profile_path,
            output_path,
        } => print_control_panel(&profile_path, output_path.as_deref()),
        UiCommand::Help => {
            print_help();
            Ok(())
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum UiCommand {
    Status {
        endpoint: LocalIpcEndpoint,
    },
    Ping {
        endpoint: LocalIpcEndpoint,
    },
    LayoutEditor {
        profile_path: PathBuf,
        output_path: Option<PathBuf>,
    },
    ControlPanel {
        profile_path: PathBuf,
        output_path: Option<PathBuf>,
    },
    Help,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Args {
    command: UiCommand,
}

impl Args {
    fn parse(mut args: impl Iterator<Item = String>) -> Result<Self, String> {
        let Some(command) = args.next() else {
            return Ok(Self {
                command: UiCommand::Status {
                    endpoint: default_local_ipc_endpoint(),
                },
            });
        };

        match command.as_str() {
            "status" => Ok(Self {
                command: UiCommand::Status {
                    endpoint: parse_endpoint(args.next()),
                },
            }),
            "ping" => Ok(Self {
                command: UiCommand::Ping {
                    endpoint: parse_endpoint(args.next()),
                },
            }),
            "layout-editor" => {
                let profile_path = args
                    .next()
                    .ok_or_else(|| "layout-editor requires <profile.json>".to_string())?;
                Ok(Self {
                    command: UiCommand::LayoutEditor {
                        profile_path: PathBuf::from(profile_path),
                        output_path: args.next().map(PathBuf::from),
                    },
                })
            }
            "control-panel" => {
                let profile_path = args
                    .next()
                    .ok_or_else(|| "control-panel requires <profile.json>".to_string())?;
                Ok(Self {
                    command: UiCommand::ControlPanel {
                        profile_path: PathBuf::from(profile_path),
                        output_path: args.next().map(PathBuf::from),
                    },
                })
            }
            "help" | "--help" | "-h" => Ok(Self {
                command: UiCommand::Help,
            }),
            other => Err(format!("unknown command '{other}'")),
        }
    }
}

fn parse_endpoint(address: Option<String>) -> LocalIpcEndpoint {
    let mut endpoint = default_local_ipc_endpoint();
    if let Some(address) = address {
        endpoint.address = address;
    }
    endpoint
}

fn print_status(endpoint: &LocalIpcEndpoint) -> Result<(), String> {
    let response = request(endpoint, &LocalIpcRequest::Status)?;
    print!("{}", render_status_response(endpoint, &response)?);
    Ok(())
}

fn print_ping(endpoint: &LocalIpcEndpoint) -> Result<(), String> {
    let nonce = 1;
    match request(endpoint, &LocalIpcRequest::Ping { nonce })? {
        LocalIpcResponse::Pong {
            nonce: response_nonce,
        } if response_nonce == nonce => {
            println!(
                "core_service=reachable local_ipc_transport={} local_ipc_address={}",
                endpoint.transport.as_str(),
                endpoint.address
            );
            Ok(())
        }
        LocalIpcResponse::Error { code, message } => {
            Err(format!("core service returned {code}: {message}"))
        }
        response => Err(format!("unexpected core service response: {response:?}")),
    }
}

fn print_layout_editor(profile_path: &Path, output_path: Option<&Path>) -> Result<(), String> {
    let profile_text = fs::read_to_string(profile_path)
        .map_err(|error| format!("failed to read {}: {error}", profile_path.display()))?;
    let html = layout_editor::render_layout_editor(&profile_text)?;
    write_or_print_html("layout_editor", output_path, html)
}

fn print_control_panel(profile_path: &Path, output_path: Option<&Path>) -> Result<(), String> {
    let profile_text = fs::read_to_string(profile_path)
        .map_err(|error| format!("failed to read {}: {error}", profile_path.display()))?;
    let html = control_panel::render_control_panel(&profile_text)?;
    write_or_print_html("control_panel", output_path, html)
}

fn write_or_print_html(
    label: &str,
    output_path: Option<&Path>,
    html: String,
) -> Result<(), String> {
    if let Some(output_path) = output_path {
        if let Some(parent) = output_path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)
                .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
        }
        fs::write(output_path, html)
            .map_err(|error| format!("failed to write {}: {error}", output_path.display()))?;
        println!("{label}={}", output_path.display());
    } else {
        print!("{html}");
    }
    Ok(())
}

fn request(
    endpoint: &LocalIpcEndpoint,
    request: &LocalIpcRequest,
) -> Result<LocalIpcResponse, String> {
    let mut client = LocalIpcClient::connect(endpoint).map_err(|error| error.to_string())?;
    client.request(request).map_err(|error| error.to_string())
}

fn render_status_response(
    endpoint: &LocalIpcEndpoint,
    response: &LocalIpcResponse,
) -> Result<String, String> {
    match response {
        LocalIpcResponse::Status {
            service,
            version,
            input_hot_path,
            platform_transport,
        } => Ok(format!(
            "KMSync\ncore_service={service}\nversion={version}\ninput_hot_path={input_hot_path}\nlocal_ipc_transport={platform_transport}\nlocal_ipc_address={}\n",
            endpoint.address
        )),
        LocalIpcResponse::Error { code, message } => {
            Err(format!("core service returned {code}: {message}"))
        }
        response => Err(format!("unexpected core service response: {response:?}")),
    }
}

fn print_help() {
    println!("Usage:");
    println!("  kmsync status [endpoint]");
    println!("  kmsync ping [endpoint]");
    println!("  kmsync layout-editor <profile.json> [output.html]");
    println!("  kmsync control-panel <profile.json> [output.html]");
}

#[cfg(test)]
mod tests {
    use super::*;
    use kmsync_core::local_ipc::LocalIpcTransport;

    #[test]
    fn default_command_queries_core_service_status() {
        let args = Args::parse(std::iter::empty()).expect("parse default ui args");

        match args.command {
            UiCommand::Status { endpoint } => {
                assert_eq!(endpoint, default_local_ipc_endpoint());
            }
            _ => panic!("expected status command"),
        }
    }

    #[test]
    fn control_commands_accept_custom_endpoint() {
        let args = Args::parse(["status", "custom-endpoint"].into_iter().map(String::from))
            .expect("parse status command");

        match args.command {
            UiCommand::Status { endpoint } => {
                assert_eq!(endpoint.address, "custom-endpoint");
            }
            _ => panic!("expected status command"),
        }
    }

    #[test]
    fn renders_core_service_status_without_input_event_path() {
        let endpoint = LocalIpcEndpoint::new(
            LocalIpcTransport::UnixDomainSocket,
            "/tmp/kmsync-core-service.sock",
        );
        let response = LocalIpcResponse::Status {
            service: "kmsync".to_string(),
            version: "0.1.0".to_string(),
            input_hot_path: "not_on_local_ipc".to_string(),
            platform_transport: "unix_domain_socket".to_string(),
        };

        let output = render_status_response(&endpoint, &response).expect("render status");

        assert!(output.contains("KMSync"));
        assert!(output.contains("core_service=kmsync"));
        assert!(output.contains("input_hot_path=not_on_local_ipc"));
        assert!(output.contains("local_ipc_transport=unix_domain_socket"));
        assert!(!output.contains("InputEvent"));
    }

    #[test]
    fn layout_editor_command_accepts_profile_and_output_path() {
        let args = Args::parse(
            [
                "layout-editor",
                "configs/mac-to-windows.profile.json",
                "target/kmsync-layout.html",
            ]
            .into_iter()
            .map(String::from),
        )
        .expect("parse layout editor command");

        match args.command {
            UiCommand::LayoutEditor {
                profile_path,
                output_path,
            } => {
                assert_eq!(
                    profile_path,
                    PathBuf::from("configs/mac-to-windows.profile.json")
                );
                assert_eq!(
                    output_path,
                    Some(PathBuf::from("target/kmsync-layout.html"))
                );
            }
            _ => panic!("expected layout editor command"),
        }
    }
}
