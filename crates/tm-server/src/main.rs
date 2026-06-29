use std::{net::SocketAddr, sync::Arc};

use tm_persona::PersonaConfig;

use tm_server::{
    AppState, AuthConfig, CodingBackend, EchoChatRunner, InMemoryStore, OmpAcpBackend,
    OmpAcpConfig, PostgresStore, StoreMemoryProvider, app,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr: SocketAddr = std::env::var("TM_SERVER_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:8787".to_string())
        .parse()?;
    let persona = std::env::var_os("TM_PERSONA_PATH")
        .map(PersonaConfig::from_path)
        .unwrap_or_default();
    let chat = Arc::new(EchoChatRunner);

    if let Ok(dsn) = std::env::var("TM_DATABASE_URL") {
        let store = Arc::new(PostgresStore::connect(&dsn).await?);
        let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
        let state = configure_coding_backend(AppState::new(
            store,
            memory,
            chat,
            persona,
            AuthConfig::NoAuth,
        ))?;
        serve(addr, state).await?;
    } else {
        tracing::warn!(
            "TM_DATABASE_URL not set; using non-durable in-memory store for local development"
        );
        let store = Arc::new(InMemoryStore::default());
        let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
        let state = configure_coding_backend(AppState::new(
            store,
            memory,
            chat,
            persona,
            AuthConfig::NoAuth,
        ))?;
        serve(addr, state).await?;
    }

    Ok(())
}

fn configure_coding_backend<S, M, C>(
    mut state: AppState<S, M, C>,
) -> Result<AppState<S, M, C>, Box<dyn std::error::Error>> {
    if let Some(root) = std::env::var_os("TM_OMP_ACP_ARTIFACT_ROOT") {
        state = state.with_artifact_root(root);
    }
    if std::env::var("TM_OMP_ACP_ENABLED").ok().as_deref() == Some("1") {
        let backend: Arc<dyn CodingBackend> = Arc::new(OmpAcpBackend::new(
            OmpAcpConfig::from_env()?,
            Arc::clone(&state.approval_broker),
        )?);
        state = state.with_coding_backend(backend);
    }
    Ok(state)
}

async fn serve<S, M, C>(
    addr: SocketAddr,
    state: AppState<S, M, C>,
) -> Result<(), Box<dyn std::error::Error>>
where
    S: tm_server::Store,
    M: tm_server::MemoryProvider,
    C: tm_server::ChatRunner,
{
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app(state)).await?;
    Ok(())
}
