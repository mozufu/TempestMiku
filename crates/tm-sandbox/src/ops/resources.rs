use super::*;

#[op2]
#[serde]
pub(super) async fn op_tm_host_call(
    state: Rc<RefCell<OpState>>,
    #[string] name: String,
    #[serde] args: serde_json::Value,
) -> serde_json::Value {
    let host_state = {
        let state = state.borrow();
        state.borrow::<RuntimeHostState>().clone()
    };
    sdk_result(
        host_state
            .host_registry
            .invoke(&name, args, &host_state.invocation_ctx)
            .await,
    )
}

#[op2]
#[serde]
pub(super) async fn op_tm_resource_read(
    state: Rc<RefCell<OpState>>,
    #[string] uri: String,
    #[string] selector: String,
) -> serde_json::Value {
    let host_state = {
        let state = state.borrow();
        state.borrow::<RuntimeHostState>().clone()
    };
    let selector = (!selector.is_empty()).then_some(selector);
    sdk_result(
        host_state
            .resource_registry
            .read(&uri, selector.as_deref(), &host_state.invocation_ctx)
            .await,
    )
}

#[op2]
#[serde]
pub(super) async fn op_tm_resource_preview(
    state: Rc<RefCell<OpState>>,
    #[string] uri: String,
) -> serde_json::Value {
    let host_state = {
        let state = state.borrow();
        state.borrow::<RuntimeHostState>().clone()
    };
    sdk_result(
        host_state
            .resource_registry
            .preview(&uri, &host_state.invocation_ctx)
            .await,
    )
}

#[op2]
#[serde]
pub(super) async fn op_tm_resource_list(
    state: Rc<RefCell<OpState>>,
    #[string] uri: String,
) -> serde_json::Value {
    let host_state = {
        let state = state.borrow();
        state.borrow::<RuntimeHostState>().clone()
    };
    let uri = (!uri.is_empty()).then_some(uri);
    sdk_result(
        host_state
            .resource_registry
            .list(uri.as_deref(), &host_state.invocation_ctx)
            .await,
    )
}
