//! Minimal Redis client implementation
//!
//! Provides an async connect and methods for issuing the supported commands.

// use crate::cmd::{Get, Publish, Set, Subscribe, Unsubscribe};
use crate::{RpcFrame};

use chainpack::{RpcMessage, RpcMessageMetaTags, RpcValue};
use chainpack::rpcframe::Protocol;
use std::time::{Duration, Instant};
use async_std::{
    channel::{Sender},
    // io::{stdin, BufReader, BufWriter},
    // net::{TcpStream, ToSocketAddrs},
    //prelude::*,
    task,
    future,
};
use log::{trace, debug, info, warn, error};

const DEFAULT_RPC_CALL_TIMEOUT_MS: u64 = 5000;

#[derive(Copy, Clone)]
pub enum PasswordType {
    PLAIN,
    SHA1
}
impl PasswordType {
    pub fn to_str(&self) -> &str {
        match self {
            PasswordType::PLAIN => "PLAIN",
            PasswordType::SHA1 => "SHA1",
        }
    }
}

#[derive(Clone)]
pub struct ConnectionParams {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
    pub password_type: PasswordType,
    pub device_id: String,
    pub mount_point: String,
    pub heartbeat_interval: Option<Duration>,
    pub protocol: Protocol,
}
impl ConnectionParams {
    pub fn new(host: &str, port: u16, user: &str, password: &str) -> ConnectionParams {
        ConnectionParams {
            host: host.into(),
            port,
            user: user.into(),
            password: password.into(),
            password_type: if user.len() == 40 { PasswordType::SHA1} else { PasswordType::PLAIN },
            device_id: "".into(),
            mount_point: "".into(),
            heartbeat_interval: Some(Duration::from_secs(60)),
            protocol: Protocol::ChainPack,
        }
    }
    fn to_rpcvalue(&self) -> RpcValue {
        let mut map = chainpack::rpcvalue::Map::new();
        let mut login = chainpack::rpcvalue::Map::new();
        login.insert("user".into(), RpcValue::from(&self.user));
        login.insert("password".into(), RpcValue::from(&self.password));
        login.insert("type".into(), RpcValue::from(self.password_type.to_str()));
        map.insert("login".into(), RpcValue::from(login));
        let mut options = chainpack::rpcvalue::Map::new();
        if let Some(hbi) = self.heartbeat_interval {
            options.insert("idleWatchDogTimeOut".into(), RpcValue::from(hbi.as_secs() * 3));
        }
        let mut device = chainpack::rpcvalue::Map::new();
        if !self.device_id.is_empty() {
            device.insert("deviceId".into(), RpcValue::from(&self.device_id));
        }
        else if !self.mount_point.is_empty() {
            device.insert("mountPoint".into(), RpcValue::from(&self.mount_point));
        }
        if !device.is_empty() {
            options.insert("device".into(), RpcValue::from(device));
        }
        map.insert("options".into(), RpcValue::from(options));
        RpcValue::from(map)
    }
}

pub type ClientTx = Sender<RpcFrame>;
pub type ClientRx = async_broadcast::Receiver<RpcFrame>;

#[derive(Clone)]
pub struct ClientSender {
    pub sender: ClientTx,
    pub protocol: Protocol,
}

#[derive(Clone)]
pub struct Client {
    pub sender: ClientTx,
    pub receiver: ClientRx,
    pub protocol: Protocol,
}

impl Client {
    pub async fn login(&mut self, login_params: &ConnectionParams) -> crate::Result<()> {
        let hello_resp = self.call_rpc_method(RpcMessage::create_request("", "hello", None)).await?;
        debug!("hello resp {}", hello_resp);
        let mut login_params = login_params.clone();
        if login_params.password.len() != 40 {
            if let Some(result) = hello_resp.result() {
                if let Some(nonce) = result.as_map().get("nonce") {
                    let hash = crate::utils::sha1_password_hash(login_params.password.as_str().as_bytes(), nonce.as_str().as_bytes());
                    login_params.password = hash;
                    login_params.password_type = PasswordType::SHA1;
                } else {
                    warn!("nonce param missing!");
                }
            } else {
                warn!("hello response params missing!");
            }
        }
        let login_resp = self.call_rpc_method(RpcMessage::create_request("", "login", Some(login_params.to_rpcvalue()))).await?;
        debug!("login result: {}", login_resp);
        match login_resp.result() {
            Some(_) => {
                Ok(())
            },
            None => Err("Login incorrect!".into())
        }
    }

    pub fn spawn_ping_task(&self, heartbeat_interval: Duration) {
        let mut client = self.clone();
        task::spawn(async move {
            info!("Starting heart-beat task with period: {} sec", heartbeat_interval.as_secs());
            loop {
                task::sleep(heartbeat_interval).await;
                let ping_start = Instant::now();
                let rq = RpcMessage::create_request(".broker/app", "ping", None);
                debug!("Sending heart beat: {}", rq);
                match client.send_message(&rq).await {
                    Ok(_) => {}
                    Err(e) => error!("cannot send ping: {}", e),
                }
                loop {
                    match client.receive_message_timeout(heartbeat_interval * 2).await {
                        Ok(resp) => {
                            trace!("ping task response received: {}", resp);
                            if resp.is_response() && resp.request_id() == rq.request_id() {
                                debug!("Ping response received OK");
                                break;
                            }
                            if ping_start.elapsed() > heartbeat_interval {
                                error!("Receive ping response timeout after: {:?}", ping_start.elapsed());
                                break;
                            }
                        }
                        Err(_) => error!("Receive ping response timeout after: {:?}", ping_start.elapsed()),
                    }
                }
            }
        });
    }

    pub async fn call_rpc_method(& self, request: RpcMessage) -> crate::Result<RpcMessage> {
        if !request.is_request() {
            return Err("Not request".into())
        }
        let rq_id = request.request_id().ok_or("Request ID missing")?;
        trace!("sending RPC request id: {} msg: {}", rq_id, request);
        self.send_message(&request).await?;
        let mut client = self.clone();
        match future::timeout(Duration::from_millis(DEFAULT_RPC_CALL_TIMEOUT_MS), async move {
            loop {
                let resp = client.receive_message().await?;
                trace!(target: "rpcmsg", "{} maybe response: {}", rq_id, resp);
                if let Some(id) = resp.request_id() {
                    if id == rq_id {
                        //let resp = resp.clone();
                        trace!("{} .............. got response: {}", rq_id, resp);
                        return Ok(resp)
                    }
                }
            }
        }).await {
            Ok(resp) => resp,
            Err(_) => Err(format!("Response to request id: {} didn't arrive within {} msec.", rq_id, DEFAULT_RPC_CALL_TIMEOUT_MS).into()),
        }
    }
    async fn send_frame(& self, frame: RpcFrame) -> crate::Result<()> {
        self.sender.send(frame).await?;
        Ok(())
    }
    pub async fn receive_frame(&mut self) -> crate::Result<RpcFrame> {
        let frame = self.receiver.recv().await?;
        Ok(frame)
    }
    pub async fn send_message(& self, msg: &RpcMessage) -> crate::Result<()> {
        let frame = RpcFrame::from_rpcmessage(self.protocol, &msg)?;
        self.send_frame(frame).await?;
        Ok(())
    }
    pub async fn receive_message(&mut self) -> crate::Result<RpcMessage> {
        let frame = self.receive_frame().await?;
        let msg = frame.to_rpcmesage()?;
        return Ok(msg)
    }
    pub async fn receive_message_timeout(&mut self, timeout: Duration) -> crate::Result<RpcMessage> {
        future::timeout(timeout, self.receive_message()).await?
    }

    pub fn to_sender(& self) -> ClientSender {
        ClientSender {
            sender: self.sender.clone(),
            protocol: self.protocol,
        }
    }
}

impl ClientSender {
    pub async fn send_frame(& self, frame: RpcFrame) -> crate::Result<()> {
        self.sender.send(frame).await?;
        Ok(())
    }
    pub async fn send_message(& self, msg: &RpcMessage) -> crate::Result<()> {
        let frame = RpcFrame::from_rpcmessage(self.protocol, &msg)?;
        self.send_frame(frame).await?;
        Ok(())
    }
}
