use chainpack::metamethod::{MetaMethod, LsAttribute};
use chainpack::rpcvalue::List;
use chainpack::{RpcValue, RpcMessage, RpcMessageMetaTags};
use tracing::debug;
use async_trait::async_trait;
use crate::utils;
use std::future::Future;
use std::pin::Pin;

pub struct NodesTree {
    pub root: Option<TreeNode>,
}
impl NodesTree {
    pub async fn process_request(&mut self, request: &RpcMessage) -> crate::Result<RpcValue>  {
        if !request.is_request() {
            return Err("Not request".into());
        }
        debug!("request: {}", request);
        let shv_path = request.shv_path().unwrap_or("");
        let path = utils::split_shv_path(shv_path);
        debug!("path: {:?}", path);
        let method = request.method().ok_or("Method is empty")?;
        debug!("method: {}", method);
        let nd = self.cd(&path)?;
        let path = &path[nd.1 ..];
        let nd = nd.0;
        let params = request.params();
        if method == "dir" {
            let mut method_pattern = "".to_string();
            let mut attrs_pattern = 0;
            if let Some(params) = params {
                if params.is_list() {
                    let params = params.as_list();
                    if params.len() >= 1 {
                        method_pattern = params[0].as_str()?.to_string();
                    }
                    if params.len() >= 2 {
                        //debug!("param [1]: {}", params[1]);
                        attrs_pattern = params[1].as_u32();
                    }
                } else {
                    method_pattern = params.to_string();
                }
            }
            debug!("dir - method pattern: {}, attrs pattern: {}", method_pattern, attrs_pattern);
            return nd.dir(path, &method_pattern, attrs_pattern).await;
        } else if method == "ls" {
            let mut name_pattern = "".to_string();
            let mut ls_attrs = 0;
            if let Some(params) = params {
                if params.is_list() {
                    let params = params.as_list();
                    if params.len() >= 1 {
                        name_pattern = params[0].as_str()?.to_string();
                    }
                    if params.len() >= 2 {
                        //debug!("param [1]: {}", params[1]);
                        ls_attrs = params[1].as_u32();
                    }
                } else {
                    name_pattern = params.to_string();
                }
            }
            debug!("name pattern: {}, with_children_info: {}", name_pattern, ls_attrs);
            return nd.ls(path, &name_pattern, ls_attrs).await;
        } else {
            return nd.call_method(path, method, params).await;
        }
    }

    fn cd(&mut self, path: &[&str]) -> crate::Result<(&mut TreeNode, usize)> {
        match &mut self.root {
            None => {return Err("Root node is not set".into())}
            Some(root) => {
                let mut pnd = root;
                let mut ix = 0;
                for p in path {
                    for nd in pnd.child_nodes.iter_mut() {
                        if &nd.name == p {
                            // pnd = nd;
                            break;
                        } else {
                            // return Err("abc".into())
                            return Ok((nd, ix))
                        }
                    }
                    ix += 1;
                }
                return Ok((root, 0))
            }
        }
    }
}

pub struct TreeNode {
    pub name: String,
    pub processors: Vec<Box<dyn RpcMethodProcessor>>,
    pub child_nodes: Vec<Box<TreeNode>>,
}

impl TreeNode {
    fn add_child_node(&mut self, nd: Box<TreeNode>) -> &mut Self {
        self.child_nodes.push(nd);
        self
    }
    fn is_leaf(&self) -> bool {
        if !self.child_nodes.is_empty() {
            return false;
        }
        for p in self.processors.iter() {
            if !p.is_leaf() {
                return false;
            }
        }
        return true;
    }
    async fn dir(& self, path: &[&str], method_pattern: &str, attrs_pattern: u32) -> crate::Result<RpcValue> {
        debug!("dir method pattern: {}, attrs pattern: {}", method_pattern, attrs_pattern);
        let mut lst: List = Vec::new();
        for p in self.processors.iter() {
            for mm in p.dir(path).await? {
                if method_pattern.is_empty() {
                    lst.push(mm.dir_attributes(attrs_pattern as u8));
                }
                else if method_pattern == mm.name {
                    lst.push(mm.dir_attributes(attrs_pattern as u8));
                    break;
                }
            }
        }
        debug!("dir: {:?}", lst);
        return Ok(RpcValue::new(lst));
    }
    async fn ls(& self, path: &[&str], name_pattern: &str, ls_attrs_pattern: u32) -> crate::Result<RpcValue> {
        let with_children_info = (ls_attrs_pattern & (LsAttribute::HasChildren as u32)) != 0;
        debug!("ls name_pattern: {}, with_children_info: {}", name_pattern, with_children_info);
        let filter = |name: &str, is_leaf: bool| {
            if !name_pattern.is_empty() {
                name_pattern == name
            } else {
                true
            }
        };
        let map = |name: &str, is_leaf: bool| -> RpcValue {
            if with_children_info {
                let mut lst = List::new();
                lst.push(RpcValue::new(name));
                lst.push(RpcValue::new(!is_leaf));
                RpcValue::new(lst)
            } else {
                RpcValue::new(name)
            }
        };
        let mut lst = List::new();
        for p in self.processors.iter() {
            if !p.is_leaf() {
                for i in p.ls(path).await? {
                    if filter(&i.0, i.1) {
                        lst.push(map(&i.0, i.1));
                    }
                }
            }
        }
        for nd in self.child_nodes.iter() {
            if filter(&nd.name, nd.is_leaf()) {
                lst.push(map(&nd.name, nd.is_leaf()));
            }
        }
        // debug!("dir: {:?}", lst);
        return Ok(RpcValue::new(lst));
    }
    async fn call_method(&mut self, path: &[&str], method: &str, params: Option<&RpcValue>) -> crate::Result<RpcValue> {
        for p in self.processors.iter_mut() {
            for m in p.dir(&path).await? {
                // TDDO: check access rights
                if m.name == method {
                    return p.call_method(&path, method, params).await
                }
            }
        }
        Err(format!("Unknown method: {} on path: {:?}", method, path).into())
    }
}

#[async_trait]
pub trait RpcMethodProcessor: Send + Sync {
    async fn dir<'a>(&'a self, path: &'_[&str]) -> crate::Result<Vec<&'a MetaMethod>>;
    async fn ls(&self, path: &[&str]) -> crate::Result<Vec<(String, bool)>> {
        Ok(Vec::new())
    }
    fn is_leaf(&self) -> bool { true }
    async fn call_method(&mut self, path: &[&str], method: &str, params: Option<&RpcValue>) -> crate::Result<RpcValue>;
}