use std::{cell::RefCell, collections::BTreeMap, rc::Rc};

use async_trait::async_trait;
use deno_core::{OpState, extension, op2};
use deno_error::JsErrorBox;
use serde::Serialize;
use serde_json::{Value, json};
use tm_artifacts::{ArtifactRef, ArtifactStore};
use tm_host::{
    GrantDoc, HostError, HostFn, HostRegistry, InvocationCtx, ResourceRegistry, ToolDocs,
    ToolErrorDoc, ToolExample, ToolSummary,
};

#[derive(Clone)]
pub(crate) struct RuntimeHostState {
    pub(crate) artifact_store: ArtifactStore,
    pub(crate) host_registry: HostRegistry,
    pub(crate) resource_registry: ResourceRegistry,
    pub(crate) invocation_ctx: InvocationCtx,
}

mod artifacts;
mod docs;
mod http;
mod resources;
mod tools;

use artifacts::{op_tm_artifact_list, op_tm_artifact_put};
use docs::{core_doc_granted, core_doc_matches, core_tool_docs};
pub(crate) use http::HttpGetFn;
use resources::{
    op_tm_host_call, op_tm_resource_list, op_tm_resource_preview, op_tm_resource_read,
};
use tools::{op_tm_tools_docs, op_tm_tools_search};

fn sdk_result<T: Serialize>(result: std::result::Result<T, HostError>) -> Value {
    match result {
        Ok(value) => sdk_ok(value),
        Err(err) => json!({
            "ok": false,
            "error": err.to_payload()
        }),
    }
}

fn sdk_ok<T: Serialize>(value: T) -> Value {
    match serde_json::to_value(value) {
        Ok(value) => json!({
            "ok": true,
            "value": value
        }),
        Err(err) => json!({
            "ok": false,
            "error": HostError::HostCall(err.to_string()).to_payload()
        }),
    }
}

fn js_error(err: impl ToString) -> JsErrorBox {
    JsErrorBox::generic(err.to_string())
}

extension!(
    tm_sandbox_ops,
    ops = [
        op_tm_host_call,
        op_tm_resource_read,
        op_tm_resource_preview,
        op_tm_resource_list,
        op_tm_artifact_put,
        op_tm_artifact_list,
        op_tm_tools_search,
        op_tm_tools_docs
    ],
    options = {
        host_state: RuntimeHostState,
    },
    state = |state, options| {
        state.put(options.host_state);
    },
);

pub(crate) fn init_ops(host_state: RuntimeHostState) -> deno_core::Extension {
    tm_sandbox_ops::init(host_state)
}
