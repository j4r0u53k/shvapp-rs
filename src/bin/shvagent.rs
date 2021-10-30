use structopt::StructOpt;
use std::{env, io};
use std::time::Duration;
use chainpack::{RpcMessage, RpcMessageMetaTags, RpcValue, metamethod};

use chainpack::rpcmessage::{RpcError, RpcErrorCode};
use chainpack::rpcvalue::List;
use chainpack::metamethod::{MetaMethod};

use shvapp::{Connection, DEFAULT_PORT};
use shvapp::client::{Client, ConnectionParams};
use shvapp::shvnode::{TreeNode, NodesTree, RequestProcessor, ProcessRequestResult};
use shvapp::shvfsnode::FSDirRequestProcessor;

use log::{warn, info, debug, LevelFilter};

use fern::colors::ColoredLevelConfig;
use colored::Color;
use colored::Colorize;

use async_std::{
    process::Command,
    // channel::{Receiver, Sender},
    // io::{stdin, BufReader, BufWriter},
    net::{TcpStream},
    // prelude::*,
    task,
    // future,
};

#[derive(StructOpt, Debug)]
#[structopt(name = "shvagent", version = env!("CARGO_PKG_VERSION"), author = env!("CARGO_PKG_AUTHORS"), about = "SHV Agent")]
struct Cli {
    #[structopt(name = "hostname", short = "-s", long = "--host", default_value = "127.0.0.1")]
    host: String,
    #[structopt(short = "-p", long = "--port", default_value = DEFAULT_PORT)]
    port: u16,
    #[structopt(short = "-u", long = "--user")]
    user: String,
    #[structopt(long = "--password")]
    password: String,
    #[structopt(long = "--device-id")]
    device_id: Option<String>,
    #[structopt(short = "-m", long = "--mount-point")]
    mount_point: Option<String>,
    #[structopt(short = "-v", long = "--verbose", help = "Verbosity levels for targets, for example: rpcmsg:W or :T")]
    verbosity: Option<String>,
    #[structopt(short = "-e", long = "--export-dir", help = "Directory, which will be exported as 'fs' subnode")]
    export_dir: Option<String>,
}

// const DEFAULT_RPC_TIMEOUT_MSEC: u64 = 5000;

fn setup_logging(verbosity: &Option<String>) -> Result<Vec<(String, log::LevelFilter)>, fern::InitError> {
    let mut ret: Vec<(String, log::LevelFilter)> = Vec::new();
    let colors = ColoredLevelConfig::new()
        // use builder methods
        .error(Color::BrightRed)
        .warn(Color::BrightMagenta)
        .info(Color::Cyan)
        .debug(Color::White)
        .trace(Color::BrightBlack);

    let mut base_config = fern::Dispatch::new();
    base_config = match verbosity {
        None => {
            ret.push(("".into(), log::LevelFilter::Info));
            base_config
                .level(log::LevelFilter::Info)
        }
        Some(levels) => {
            let mut default_level_set = false;
            for level_str in levels.split(',') {
                let parts: Vec<&str> = level_str.split(':').collect();
                let (target, level_abbr) = if parts.len() == 1 {
                    (parts[0], "T")
                } else if parts.len() == 2 {
                    (parts[0], parts[1])
                } else {
                    panic!("Cannot happen");
                };
                let level = match level_abbr {
                    "D" => log::LevelFilter::Debug,
                    "I" => log::LevelFilter::Info,
                    "W" => log::LevelFilter::Warn,
                    "E" => log::LevelFilter::Error,
                    _ => log::LevelFilter::Trace,
                };
                ret.push((target.to_string(), level));
                if target.is_empty() {
                    base_config = base_config.level(level);
                    default_level_set = true;
                } else {
                    base_config = base_config.level_for(target.to_string(), level);
                }
            }
            if !default_level_set {
                base_config = base_config.level(LevelFilter::Info);
            }
            base_config
        }
    };
    base_config
        //.chain(file_config)
        .format(move |out, message, record| {
            let level_color: fern::colors::Color = colors.get_color(&record.level());
            let target = if record.module_path().unwrap_or("") == record.target() { "".to_string() } else { format!("({})", record.target().bright_white()) };
            out.finish(format_args!(
                "{}{}{}{} {}",
                chrono::Local::now().format("%Y-%m-%dT%H:%M:%S.%3f").to_string().green(),
                format!("[{}:{}]", record.module_path().unwrap_or(""), record.line().unwrap_or(0)).yellow(),
                &target,
                format!("[{}]", &record.level().as_str()[..1]).color(level_color),
                format!("{}", message).color(level_color)
            ))
        })
        .chain(io::stderr())
        .apply()?;
    Ok(ret)
}

pub(crate) fn main() -> shvapp::Result<()> {
    task::block_on(try_main())
}

async fn try_main() -> shvapp::Result<()> {
    // Parse command line arguments
    let cli = Cli::from_args();

    let levels = setup_logging(&cli.verbosity).expect("failed to initialize logging.");
    log::info!("=====================================================");
    log::info!("{} starting up!", std::module_path!());
    log::info!("=====================================================");
    log::info!("Verbosity levels: {}", levels.iter()
        .map(|(target, level)| format!("{}:{}", target, level))
        .fold(String::new(), |acc, s| if acc.is_empty() { s } else { acc + "," + &s }));

    let device_id = cli.device_id.unwrap_or("".into());
    // Get the remote address to connect to
    // let rpc_timeout = Duration::from_millis(DEFAULT_RPC_TIMEOUT_MSEC);
    let mut connection_params = ConnectionParams::new(&cli.host, cli.port, &cli.user, &cli.password);
    connection_params.device_id = device_id.to_string();
    connection_params.mount_point = cli.mount_point.unwrap_or("".to_string());

    let mut root = TreeNode {
        processor: Some(Box::new(DeviceNodeRequestProcessor {
            app_name: "ShvAgent".into(),
            device_id,
        })),
        children: None,
    };
    //let exported_dir = dirs::home_dir();
    if let Some(export_dir) = cli.export_dir {
        root.add_child_node("fs", TreeNode {
            processor: Some(Box::new(FSDirRequestProcessor {
                root: export_dir.into(),
            })),
            children: None,
        }
        );
    }
    let mut shv_tree = NodesTree::new(root);
    //let mut app_node = ShvAgentAppNode::new();
    //let dyn_app_node = (&mut app_node) as &dyn ShvNode;
    loop {
        // Establish a connection
        let addr = format!("{}:{}", connection_params.host, connection_params.port);
        info!("connecting to: {}", addr);
        let stream = TcpStream::connect(&addr).await?;
        info!("connected to: {}", addr);
        let mut connection = Connection::new(stream);
        let mut client = connection.create_client(connection_params.protocol);
        task::spawn(async move {
            info!("Spawning connection message loop");
            match connection.exec().await {
                Ok(_) => {
                    info!("Connection message loop finished Ok");
                }
                Err(e) => {
                    warn!("Connection message loop finished with error: {}", e);
                }
            }
        });
        match client.login(&connection_params).await {
            Ok(_) => {
                /*
                let ping_fut = client.call(RpcMessage::create_request(".broker/app", "ping", None));
                match ping_fut.await {
                    Ok(resp) => {
                        info!("Ping response: {}", resp);
                    }
                    Err(e) => {
                        info!("Ping error: {}", e);
                    }
                }
                */
                loop {
                    match client.receive_message().await {
                        Ok(msg) => {
                            debug!(target: "rpcmsg", "<== Message arrived: {}", msg);
                            if msg.is_request() {
                                let ret_val = shv_tree.process_request(&client,&msg);
                                if let Ok(None) = ret_val {
                                    // ret val will be sent async in handler
                                }
                                else {
                                    match msg.prepare_response() {
                                        Ok(mut resp_msg) => {
                                            match ret_val {
                                                Ok(None) => {}
                                                Ok(Some(rv)) => {
                                                    resp_msg.set_result(rv);
                                                    debug!(target: "rpcmsg", "==> Sending response: {}", &resp_msg);
                                                    client.send_message(&resp_msg).await?;
                                                }
                                                Err(e) => {
                                                    resp_msg.set_error(RpcError::new(RpcErrorCode::MethodCallException, &e.to_string()));
                                                    debug!(target: "rpcmsg", "==> Sending error: {}", &resp_msg);
                                                    client.send_message(&resp_msg).await?;
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            warn!("Create response meta error: {}.", e);
                                        }
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            warn!("Read message error: {}.", e);
                        }
                    }
                }
            }
            Err(e) => {
                info!("Login error: {}", e);
                task::sleep(Duration::from_secs(5)).await;
            }
        };
    }
}

struct DeviceNodeRequestProcessor {
    app_name: String,
    device_id: String,
}

impl RequestProcessor for DeviceNodeRequestProcessor {
    fn process_request(&mut self, client: &Client, request: &RpcMessage, shv_path: &str) -> ProcessRequestResult {
        let method = request.method().ok_or("Empty method")?;
        if shv_path.is_empty() {
            if method == "dir" {
                let methods = [
                    MetaMethod { name: "dir".into(), signature: metamethod::Signature::RetParam, flags: metamethod::Flag::None.into(), access_grant: RpcValue::new("bws"), description: "".into() },
                    MetaMethod { name: "ls".into(), signature: metamethod::Signature::RetParam, flags: metamethod::Flag::None.into(), access_grant: RpcValue::new("bws"), description: "".into() },
                    MetaMethod { name: "appName".into(), signature: metamethod::Signature::RetParam, flags: metamethod::Flag::IsGetter.into(), access_grant: RpcValue::new("bws"), description: "".into() },
                    MetaMethod { name: "deviceId".into(), signature: metamethod::Signature::RetParam, flags: metamethod::Flag::IsGetter.into(), access_grant: RpcValue::new("rd"), description: "".into() },
                    MetaMethod { name: "runCmd".into(), signature: metamethod::Signature::RetParam, flags: metamethod::Flag::None.into(), access_grant: RpcValue::new("wr"), description: "".into() },
                ];
                let mut lst = List::new();
                for mm in methods.iter() {
                    lst.push(mm.to_rpcvalue(255));
                }
                return Ok(Some(lst.into()));
            }
            if method == "appName" {
                return Ok(Some(RpcValue::from(&self.app_name)));
            }
            if method == "deviceId" {
                return Ok(Some(RpcValue::from(&self.device_id)));
            }
            if method == "runCmd" {
                let request = request.clone();
                // let shv_path = shv_path.to_string();
                let client = client.clone();
                task::spawn(async move {
                    async fn run_cmd(request: &RpcMessage) -> shvapp::Result<RpcValue> {
                        let params = request.params().ok_or("No params")?;
                        let cmd = if params.is_list() {
                            let params = params.as_list();
                            if params.is_empty() {
                                return Err("Param list is empty".into());
                            }
                            params[0].as_str()
                        }
                        else if params.is_string() {
                            params.as_str()
                        }
                        else {
                            return Err("Invalid params".into());
                        };
                        let output = Command::new(cmd)
                            //.args(args)
                            .output().await?;
                        let out: &[u8] = &output.stdout;
                        return Ok(RpcValue::from(out))
                    }
                    match request.prepare_response() {
                        Ok(mut resp_msg) => {
                            match run_cmd(&request).await {
                                Ok(rv) => { resp_msg.set_result(rv); }
                                Err(e) => { resp_msg.set_error(RpcError::new(RpcErrorCode::MethodCallException, &e.to_string())); }
                            }
                            match client.send_message(&resp_msg).await {
                                Ok(_) => {}
                                Err(e) => { warn!("Send response error: {}.", e); }
                            }
                        }
                        Err(e) => {
                            warn!("Create response error: {}.", e);
                        }
                    }
                });
                return Ok(None);
            }
        }
        Err(format!("Unknown method '{}' on path '{}'", method, shv_path).into())
    }

    fn is_dir(&self) -> bool {
        return false;
    }
}
