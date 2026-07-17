use super::*;

pub(super) struct RecordingServer {
    pub(super) base_url: String,
    pub(super) artifact_root: PathBuf,
    pub(super) store: Arc<InMemoryStore>,
    broker: Arc<ApprovalBroker>,
    pub(super) persona: tm_server::ModesConfig,
    tier: tm_host::SelfEvolutionTier,
    handle: tokio::task::JoinHandle<()>,
}

impl RecordingServer {
    pub(super) async fn start(run_root: &Path) -> Result<Self> {
        Self::start_with_tier(run_root, tm_host::SelfEvolutionTier::Conservative).await
    }

    pub(super) async fn start_with_tier(
        run_root: &Path,
        tier: tm_host::SelfEvolutionTier,
    ) -> Result<Self> {
        let artifact_root = run_root.join("server-artifacts");
        fs::create_dir_all(&artifact_root)
            .with_context(|| format!("creating {}", artifact_root.display()))?;
        let store = Arc::new(InMemoryStore::default());
        let memory = Arc::new(StoreMemoryProvider::new(store.clone()));
        let chat = Arc::new(EchoChatRunner);
        let broker = Arc::new(ApprovalBroker::default());
        let roster = Arc::new(MailboxRegistry::new());
        let linked_folders = LinkedFolders::from_configs(vec![LinkedFolderConfig {
            name: "tempestmiku".to_string(),
            path: run_root.to_path_buf(),
            mode: FsMode::Rw,
            commands: Vec::new(),
            safe_args: Vec::new(),
        }])
        .context("configuring recording-server project link")?;
        let persona = tm_server::ModesConfig::default()
            .with_managed_skills_path(artifact_root.join("managed-skills"))
            .with_managed_mode_addenda_path(artifact_root.join("managed-mode-addenda"));
        let state = AppState::new(
            Arc::clone(&store),
            memory,
            chat,
            persona.clone(),
            AuthConfig::NoAuth,
        )
        .with_auto_turn_dispatcher(true)
        .with_self_evolution_tier(tier)
        .with_approval_broker(Arc::clone(&broker))
        .with_artifact_root(artifact_root.clone())
        .with_linked_folders(linked_folders)
        .with_actor_roster(Arc::clone(&roster))
        .with_coding_backend(Arc::new(RecordingBackend {
            root: artifact_root.clone(),
            broker: Arc::clone(&broker),
            roster,
        }));
        state.wire_lifecycle_sink();
        let router = app(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .context("binding tm-e2e recording server")?;
        let addr = listener
            .local_addr()
            .context("reading recording server addr")?;
        let handle = tokio::spawn(async move {
            if let Err(err) = axum::serve(listener, router).await {
                eprintln!("tm-e2e recording server exited: {err}");
            }
        });
        Ok(Self {
            base_url: format!("http://{addr}"),
            artifact_root,
            store,
            broker,
            persona,
            tier,
            handle,
        })
    }

    pub(super) async fn run_skill_dream(&self, session_id: Uuid, message: &str) -> Result<()> {
        self.store
            .append_message(session_id, "user", message)
            .await
            .context("seeding skill dream message")?;
        self.store
            .end_session_and_enqueue_dream(session_id, "brian".to_string(), "global".to_string())
            .await
            .context("ending session for skill dream")?;
        let senders = Arc::new(Mutex::new(BTreeMap::<
            Uuid,
            broadcast::Sender<tm_server::SessionEvent>,
        >::new()));
        let sender_for: tm_server::SenderFactory = Arc::new(move |session_id| {
            let mut senders = senders.lock().expect("tm-e2e dream sender lock");
            senders
                .entry(session_id)
                .or_insert_with(|| broadcast::channel(64).0)
                .clone()
        });
        let report = ServerDreamWorker::new(
            Arc::clone(&self.store),
            Arc::clone(&self.broker),
            sender_for,
            tm_server::DreamWorkerConfig {
                proposal_timeout: Duration::from_secs(5),
                ..tm_server::DreamWorkerConfig::default()
            },
        )
        .with_self_evolution_tier(self.tier)
        .run_once_result()
        .await
        .context("running skill dream")?;
        ensure!(
            report.completed == 1,
            "skill dream did not complete: {report:?}"
        );
        Ok(())
    }
}

impl Drop for RecordingServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}
