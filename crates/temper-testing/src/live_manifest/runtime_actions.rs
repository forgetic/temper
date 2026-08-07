use super::*;

impl LiveExecutionContext<'_> {
    pub(super) fn provision_forgejo(&mut self) -> Result<(), String> {
        require(self.server.is_none(), "Forgejo is already provisioned")?;
        self.harness.scenario.validate_workflow()?;
        let cached = start_cached_bare_admin_server(
            &self.harness.admin_user,
            &self.harness.admin_password,
            &self.harness.admin_email,
        )
        .map_err(|error| format!("cached bare-admin Forgejo starts: {error}"))?;
        self.forge_cache_hit = cached.cache_hit;
        self.admin_token = Some(mint_site_admin_token(
            &cached.server,
            &self.harness.admin_user,
        )?);
        self.server = Some(cached.server);
        Ok(())
    }

    pub(super) fn await_runner(&mut self) -> Result<(), String> {
        let server = required_ref(&self.server, "forgejo.provision")?;
        require(self.runner.is_none(), "forgejo-runner is already ready")?;
        let mut runner = ForgejoRunner::register(server)
            .map_err(|error| format!("forgejo-runner registers: {error}"))?;
        if !runner.is_running() {
            return Err(format!(
                "forgejo-runner exited immediately\n--- runner log ---\n{}",
                runner.log_tail()
            ));
        }
        self.runner = Some(runner);
        Ok(())
    }

    pub(super) fn start_mcp(
        &mut self,
        project: &str,
        safe_tools: &[String],
        hidden_tools: &[String],
        readiness_delay_ms: u64,
        forced_systemic_failure: Option<&super::super::ForcedSystemicFailureFixture>,
    ) -> Result<(), String> {
        required_ref(&self.server, "forgejo.provision")?;
        require(
            self.mcp.is_none(),
            "fake codebase-memory MCP is already started",
        )?;
        require(!project.trim().is_empty(), "MCP project must not be empty")?;
        require(
            !safe_tools.is_empty(),
            "MCP safe tool list must not be empty",
        )?;
        require(
            !hidden_tools.is_empty(),
            "MCP hidden tool list must not be empty",
        )?;
        let supported = [
            "search_code",
            "search_graph",
            "list_projects",
            "index_status",
            "index_repository",
            "delete_project",
        ];
        if let Some(tool) = safe_tools
            .iter()
            .chain(hidden_tools)
            .find(|tool| !supported.contains(&tool.as_str()))
        {
            return Err(format!(
                "fake codebase-memory MCP does not implement `{tool}`"
            ));
        }
        if let Some(tool) = safe_tools.iter().find(|tool| hidden_tools.contains(tool)) {
            return Err(format!(
                "MCP tool `{tool}` cannot be both model-safe and hidden"
            ));
        }
        if let Some(failure) = forced_systemic_failure {
            require(
                safe_tools.contains(&failure.tool),
                &format!(
                    "forced systemic failure tool `{}` must be a model-safe MCP tool",
                    failure.tool
                ),
            )?;
        }
        self.mcp = Some(super::super::codebase_memory::write_fake_mcp(
            self.workspace.path(),
            project,
            safe_tools,
            hidden_tools,
            readiness_delay_ms,
            forced_systemic_failure,
        )?);
        Ok(())
    }

    pub(super) fn configure_agent_tools(
        &mut self,
        role: &str,
        tool: &str,
        mode: &str,
        index: &str,
        tool_timeout_secs: Option<u64>,
        server_step: &str,
    ) -> Result<(), String> {
        required_ref(&self.mcp, "mcp.fake_codebase_memory.start")?;
        require(
            self.harness
                .scenario
                .execution
                .agents
                .iter()
                .any(|agent| agent.role == role),
            &format!("agent.tools.configure references undeclared role `{role}`"),
        )?;
        require(
            tool == "codebase_memory",
            &format!("unsupported agent tool `{tool}`"),
        )?;
        require(
            server_step.starts_with("$step:"),
            "agent tool server must reference an executed $step:<id>",
        )?;
        self.tool_configuration = Some(ToolConfiguration {
            role: role.to_string(),
            tool: tool.to_string(),
            mode: mode.to_string(),
            index: index.to_string(),
            tool_timeout_secs,
        });
        Ok(())
    }

    pub(super) fn start_jig(
        &mut self,
        script_path: &Path,
        roles: &[String],
        late_stream_failure: Option<&super::super::LateStreamFailureFixture>,
    ) -> Result<(), String> {
        required_ref(&self.server, "forgejo.provision")?;
        if let Some(fake) = &self.fake {
            require(
                fake.script_path() == script_path,
                &format!(
                    "all jig.fake_llm actions must share one script; started {}, got {}",
                    fake.script_path().display(),
                    script_path.display()
                ),
            )?;
        } else {
            self.fake = Some(ManifestFake::start(
                self.harness.scenario.execution.convergence,
                script_path,
                late_stream_failure,
            )?);
        }
        for role in roles {
            require(
                self.harness
                    .scenario
                    .execution
                    .agents
                    .iter()
                    .any(|agent| agent.role == *role && agent.kind == "llm"),
                &format!("Jig action references undeclared LLM role `{role}`"),
            )?;
            self.jig_roles.insert(role.clone());
        }
        Ok(())
    }

    pub(super) fn launch_temper(&mut self, workflow_path: &Path) -> Result<(), String> {
        let server = required_ref(&self.server, "forgejo.provision")?;
        required_ref(&self.runner, "forgejo_runner.ready")?;
        let fake = required_ref(&self.fake, "jig.fake_llm")?;
        require(
            workflow_path == self.harness.scenario.workflow_path,
            &format!(
                "declared workflow {} does not match resolved workflow {}",
                workflow_path.display(),
                self.harness.scenario.workflow_path.display()
            ),
        )?;
        require(
            self.standalone.is_none(),
            "standalone Temper is already running",
        )?;

        let bind_port = free_port()?;
        run_temper_init(TemperInitRequest {
            temper: &self.harness.temper,
            server,
            scenario: &self.harness.scenario,
            bundle_dir: &self.bundle_dir,
            workspaces_dir: &self.workspaces_dir,
            bind_port,
            fake_llm_url: &fake.base_url(),
            log: &self.logs.init_log,
            admin_user: &self.harness.admin_user,
            admin_password: &self.harness.admin_password,
            scenario_run_id: &self.scenario_run_id,
        })?;
        assert_init_workflow_yaml_matches(
            &self.bundle_dir.join("workflow.yaml"),
            &self.harness.scenario,
        )?;
        let failure_evidence = self
            .harness
            .scenario
            .ci_failure_evidence
            .as_ref()
            .map(|fixture| {
                super::super::failure_evidence::FailureEvidenceServer::start(
                    fixture.clone(),
                    self.workspace.join("logs/ci-failure-evidence.jsonl"),
                )
            })
            .transpose()?;
        tune_init_config(
            &self.bundle_dir.join("config.toml"),
            self.harness.scenario.poll_cadence.as_secs(),
            self.harness.scenario.ci_poll_cadence.as_secs(),
            self.harness.scenario.mechanical_cadence.as_secs(),
            self.harness.scenario.recovery.as_ref(),
        )?;
        if let Some(evidence) = &failure_evidence {
            evidence.configure_temper_bundle(&self.bundle_dir)?;
            evidence.configure_repository_actions(
                server.base_url(),
                required_ref(&self.admin_token, "forgejo.provision")?,
                &self.harness.scenario.repo,
            )?;
        }
        if let Some(configuration) = &self.tool_configuration {
            let mcp = required_ref(&self.mcp, "mcp.fake_codebase_memory.start")?;
            super::super::codebase_memory::tune_codebase_memory_config(
                &self.bundle_dir.join("config.toml"),
                mcp,
                configuration,
            )?;
        }
        let mut standalone = spawn_temper_standalone(
            &self.harness.temper,
            &self.bundle_dir,
            &self.logs.standalone_log,
            &self.harness.scenario.observability,
            &self.scenario_run_id,
        )?;
        wait_for_standalone(&mut standalone)?;

        let admin_token = required_ref(&self.admin_token, "forgejo.provision")?;
        let forge = if let Some(evidence) = &failure_evidence {
            super::super::convergence::admin_forge_with_evidence(
                server.base_url(),
                admin_token,
                &self.harness.scenario.repo,
                evidence.backend_config()?,
            )
        } else {
            super::super::convergence::admin_forge(
                server.base_url(),
                admin_token,
                &self.harness.scenario.repo,
            )
        };
        let repository = engine_block_on(super::super::convergence::repository(
            &forge,
            &self.harness.scenario.repo,
        ))?;
        self.forge = Some(forge);
        self.repository = Some(repository);
        self.standalone = Some(standalone);
        self.failure_evidence = failure_evidence;
        Ok(())
    }

    pub(super) fn seed_repository(
        &mut self,
        repo_id: &str,
        seed_path: &Path,
        ci_source_path: &Path,
    ) -> Result<(), String> {
        let server = required_ref(&self.server, "forgejo.provision")?;
        required_ref(&self.standalone, "temper.launch_standalone")?;
        let token = required_ref(&self.admin_token, "forgejo.provision")?;
        let repo = &self.harness.scenario.repo;
        require(
            repo_id == repo.id,
            &format!("unknown runtime repository `{repo_id}`"),
        )?;
        require(
            seed_path == repo.seed_path,
            &format!(
                "repo.seed path {} does not match resolved fixture",
                seed_path.display()
            ),
        )?;
        require(
            ci_source_path == repo.ci_source_path,
            &format!(
                "repo.seed CI source {} does not match resolved fixture",
                ci_source_path.display()
            ),
        )?;
        populate_repo(
            server.base_url(),
            token,
            self.workspace.path(),
            repo,
            &self.logs.repo_populate_log,
        )?;
        let forge = required_ref(&self.forge, "temper.launch_standalone")?;
        let repository = required_ref(&self.repository, "temper.launch_standalone")?;
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            match engine_block_on(forge.get_branch_head(repository, &repo.default_branch)) {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < deadline => {}
                Err(_) if Instant::now() < deadline => {}
                Ok(None) => {
                    return Err(format!(
                        "seeded default branch `{}` was not visible through Forgejo within 30s",
                        repo.default_branch
                    ));
                }
                Err(error) => {
                    return Err(format!(
                        "read seeded default branch `{}` through Forgejo: {error}",
                        repo.default_branch
                    ));
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        self.initial_default_branch_sha = Some(super::super::plan_feature::local_checkout_head(
            &self.workspace.path().join("repo-seed").join(&repo.name),
        )?);
        Ok(())
    }

    pub(super) fn seed_terminal_history(
        &mut self,
        fixture: &super::super::TerminalHistorySeedFixture,
    ) -> Result<(), String> {
        require(
            self.terminal_history.is_none(),
            "terminal history has already been seeded",
        )?;
        require(
            fixture.repo_id == self.harness.scenario.repo.id,
            &format!(
                "history.seed_terminal references unavailable repository `{}`",
                fixture.repo_id
            ),
        )?;
        required_mut(&mut self.standalone, "temper.launch_standalone")?.kill_and_wait()?;
        // Every history row is created while the listener is absent. Forgejo
        // may attempt delivery, but no targeted wake can reach Temper.
        let pre_seed_log = self
            .logs
            .standalone_log
            .with_file_name("standalone.before-history-seed.log");
        std::fs::copy(&self.logs.standalone_log, &pre_seed_log).map_err(|error| {
            format!(
                "archive pre-history standalone log {} to {}: {error}",
                self.logs.standalone_log.display(),
                pre_seed_log.display()
            )
        })?;

        let result = super::super::terminal_history::seed(
            required_ref(&self.forge, "temper.launch_standalone")?,
            required_ref(&self.repository, "temper.launch_standalone")?,
            &self.harness.scenario,
            fixture,
        )?;
        self.issues
            .insert(fixture.actionable_issue_id.clone(), result.actionable_issue);
        self.issue_order.push(result.actionable_issue);
        self.terminal_history = Some(result.evidence);

        let mut standalone = spawn_temper_standalone(
            &self.harness.temper,
            &self.bundle_dir,
            &self.logs.standalone_log,
            &self.harness.scenario.observability,
            &self.scenario_run_id,
        )?;
        wait_for_standalone(&mut standalone)?;
        self.standalone = Some(standalone);
        Ok(())
    }
}
