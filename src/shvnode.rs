use chainpack::metamethod::{MetaMethod, LsAttribute};
use chainpack::rpcvalue::List;
use chainpack::{RpcValue, RpcMessage, RpcMessageMetaTags};
use tracing::debug;
use async_trait::async_trait;
use crate::utils;

// #[derive(Debug)]
// pub struct ShvError {
//     pub message: String
// }
//
// impl ShvError {
//     fn new(msg: &str) -> ShvError {
//         ShvError{ message: msg.to_string()}
//     }
// }
//
// impl fmt::Display for ShvError {
//     fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
//         write!(f,"{}",self.message)
//     }
// }
//
// impl Error for ShvError {
//     fn description(&self) -> &str {
//         &self.message
//     }
// }

#[async_trait]
pub trait ShvNode: Send + Sync {
    fn methods(&self, path: &[&str]) -> Vec<&'_ MetaMethod>;
    fn children(&self, path: &[&str]) -> Vec<(&'_ str, Option<bool>)>;

    async fn process_request(&mut self, request: &RpcMessage) -> crate::Result<RpcValue>  {
        if !request.is_request() {
            return Err("Not request".into());
        }
        debug!("request: {}", request);
        let shv_path = request.shv_path().unwrap_or("");
        let path = utils::split_shv_path(shv_path);
        debug!("path: {:?}", path);
        let method = request.method().ok_or("Method is empty")?;
        debug!("method: {}", method);
        let params = request.params();
        self.call_method(&path, method, params).await
    }
    async fn call_method(&mut self, path: &[&str], method: &str, params: Option<&RpcValue>) -> crate::Result<RpcValue>;

    fn dir(& self, path: &[&str], method_pattern: &str, attrs_pattern: u32) -> RpcValue {
        debug!("dir method pattern: {}, attrs pattern: {}", method_pattern, attrs_pattern);
        let mut lst: List = Vec::new();
        for mm in self.methods(path) {
            if method_pattern.is_empty() {
                lst.push(mm.dir_attributes(attrs_pattern as u8));
            }
            else if method_pattern == mm.name {
                lst.push(mm.dir_attributes(attrs_pattern as u8));
                break;
            }
        }
        debug!("dir: {:?}", lst);
        return RpcValue::new(lst);
    }
    fn ls(& self, path: &[&str], name_pattern: &str, ls_attrs_pattern: u32) -> RpcValue {
        let with_children_info = (ls_attrs_pattern & (LsAttribute::HasChildren as u32)) != 0;
        debug!("ls name_pattern: {}, with_children_info: {}", name_pattern, with_children_info);
        let filter = |it: &'_ &(&str, Option<bool>)| {
            if !name_pattern.is_empty() {
                name_pattern == it.0
            } else {
                true
            }
        };
        let map = |it: & (&str, Option<bool>)| -> RpcValue {
            if with_children_info {
                let mut lst = List::new();
                lst.push(RpcValue::new(it.0));
                match it.1 {
                    Some(b) => lst.push(RpcValue::new(b)),
                    None => lst.push(RpcValue::new(())),
                }
                RpcValue::new(lst)
            } else {
                RpcValue::new(it.0)
            }
        };
        let lst: List = self.children(path).iter().filter(filter).map(map).collect();
        debug!("dir: {:?}", lst);
        return RpcValue::new(lst);
    }
}