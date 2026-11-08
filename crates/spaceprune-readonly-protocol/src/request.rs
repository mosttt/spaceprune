//! Host-owned request files, operation binding and bounded response reads.

use super::*;

/// 桌面宿主创建并持有的一次性 Helper 请求。
pub struct PreparedRequest {
    pub(super) descriptor: HelperRequest,
    pub(super) context: OperationContext,
    #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
    pub(super) scope: spaceprune_access::CapabilityScope,
    pub(super) _directory: tempfile::TempDir,
    pub(super) request_path: PathBuf,
    pub(super) listener: TcpListener,
    pub(super) nonce: String,
    pub(super) source_id: String,
}

impl PreparedRequest {
    pub(super) fn requires_developer_handlers(&self) -> bool {
        self.descriptor
            .scan_task
            .as_ref()
            .and_then(|value| value.get("task"))
            .and_then(|value| value.get("handler_id"))
            .and_then(serde_json::Value::as_str)
            .is_some_and(|handler| handler.starts_with("builtin.developer-tools/"))
    }

    /// Bind a prepared one-time request to its caller before authorization or transport starts.
    pub fn with_operation(mut self, operation: &OperationContext) -> Result<Self, HelperError> {
        self.context = operation.child(REQUEST_LIFETIME);
        let mut request = self.descriptor.clone();
        request.context = self.context.transfer().map_err(|_| HelperError)?;
        let mut file =
            tempfile::NamedTempFile::new_in(self._directory.path()).map_err(|_| HelperError)?;
        file.write_all(&encode_request(&request)?)
            .map_err(|_| HelperError)?;
        file.flush().map_err(|_| HelperError)?;
        secure_created_file(file.path())?;
        file.persist(&self.request_path).map_err(|_| HelperError)?;
        self.descriptor = request;
        Ok(self)
    }

    /// 为一个已登记来源创建私有、短期且单次消费的请求。
    pub fn create(source_id: &str, roots: &[PathBuf]) -> Result<Self, HelperError> {
        Self::create_for_operation(source_id, roots, HelperOperation::MeasureRegisteredSource)
    }

    /// 为宿主登记的单个 AI 导航权限边界创建私有、短期且单次消费的请求。
    pub fn create_navigation_boundary(boundary_id: &str, root: &Path) -> Result<Self, HelperError> {
        Self::create_for_operation(
            boundary_id,
            &[root.to_path_buf()],
            HelperOperation::MeasureNavigationBoundary,
        )
    }

    pub(super) fn create_for_operation(
        source_id: &str,
        roots: &[PathBuf],
        operation: HelperOperation,
    ) -> Result<Self, HelperError> {
        Self::create_with_scan(source_id, roots, operation, None)
    }

    /// Prepare a scoped built-in filesystem metadata scan, never a cleanup request.
    #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
    pub fn create_filesystem_scan(
        source_id: &str,
        task: &spaceprune_plugin_api::TaskRequest,
    ) -> Result<Self, HelperError> {
        let roots = filesystem_scan_roots(task)?;
        Self::create_with_scan(
            source_id,
            &roots,
            HelperOperation::ScanFilesystemTask,
            Some(serde_json::to_value(task).map_err(|_| HelperError)?),
        )
    }

    /// Prepare a read-only snapshot inventory and size request for the shared helper.
    #[cfg(target_os = "linux")]
    pub fn create_snapper_scan(
        source_id: &str,
        task: &spaceprune_plugin_api::TaskRequest,
    ) -> Result<Self, HelperError> {
        snapper::validate_task(task)?;
        Self::create_with_scan(
            source_id,
            &[],
            HelperOperation::ScanSnapperTask,
            Some(serde_json::to_value(task).map_err(|_| HelperError)?),
        )
    }

    /// Preview installed systemd cleanup rules without permitting a command or custom configuration.
    #[cfg(target_os = "linux")]
    pub fn create_tmpfiles_scan(
        source_id: &str,
        task: &spaceprune_plugin_api::TaskRequest,
    ) -> Result<Self, HelperError> {
        spaceprune_plugin_system_linux::validate_tmpfiles_task(task).map_err(|_| HelperError)?;
        Self::create_with_scan(
            source_id,
            &[],
            HelperOperation::ScanTmpfilesPolicy,
            Some(serde_json::to_value(task).map_err(|_| HelperError)?),
        )
    }

    /// Keep the invoking user's discovered cache roots and identities during native retry.
    #[cfg(target_os = "linux")]
    pub fn create_system_cache_scan(
        source_id: &str,
        request: &spaceprune_plugin_api::SystemCacheScanRequest,
    ) -> Result<Self, HelperError> {
        let roots = spaceprune_plugin_system_linux::validate_system_cache_scan(request)
            .map_err(|_| HelperError)?;
        Self::create_with_scan(
            source_id,
            &roots,
            HelperOperation::ScanSystemCacheTask,
            Some(serde_json::to_value(request).map_err(|_| HelperError)?),
        )
    }

    #[cfg(target_os = "linux")]
    pub(super) fn create_snapper_inventory(config: Option<&str>) -> Result<Self, HelperError> {
        let task = spaceprune_plugin_api::TaskRequest {
            module_id: "system".to_owned(),
            task_id: "snapper-inventory".to_owned(),
            name: "Snapper inventory".to_owned(),
            handler_id: "builtin.snapper/snapshots".to_owned(),
            options: serde_json::json!({"scan-mode": "snapshots", "configs": config.into_iter().collect::<Vec<_>>() }),
            recommend: serde_json::json!({}),
            tags: vec![],
        };
        snapper::validate_task(&task)?;
        Self::create_with_scan(
            "recipe-source-0",
            &[],
            HelperOperation::InspectSnapperTask,
            Some(serde_json::to_value(task).map_err(|_| HelperError)?),
        )
    }

    fn create_with_scan(
        source_id: &str,
        roots: &[PathBuf],
        operation: HelperOperation,
        scan_task: Option<serde_json::Value>,
    ) -> Result<Self, HelperError> {
        if !valid_source_id(source_id)
            || !request_root_shape_is_valid(operation, roots)
            || !roots.iter().all(|root| {
                root.is_absolute()
                    && !root
                        .components()
                        .any(|component| component == Component::ParentDir)
            })
        {
            return Err(HelperError);
        }

        let directory = tempfile::Builder::new()
            .prefix("spaceprune-readonly-")
            .tempdir()
            .map_err(|_| HelperError)?;
        tighten_directory_permissions(directory.path())?;
        let nonce = random_nonce()?;
        let request_path = directory.path().join(format!("request-{nonce}.json"));
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).map_err(|_| HelperError)?;
        listener.set_nonblocking(true).map_err(|_| HelperError)?;
        let response_port = listener.local_addr().map_err(|_| HelperError)?.port();
        let now = unix_timestamp()?;
        #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
        let scope = client::request_scope(roots, scan_task.as_ref());
        let context = OperationContext::default().child(REQUEST_LIFETIME);
        let request = HelperRequest {
            schema_version: REQUEST_SCHEMA_VERSION,
            context: context.transfer().map_err(|_| HelperError)?,
            operation,
            source_id: source_id.to_owned(),
            roots: roots.to_vec(),
            response_port,
            nonce: nonce.clone(),
            issued_at: now,
            expires_at: now.saturating_add(REQUEST_LIFETIME.as_secs()),
            scan_task,
        };
        let mut file = private_file_options()
            .create_new(true)
            .write(true)
            .open(&request_path)
            .map_err(|_| HelperError)?;
        file.write_all(&encode_request(&request)?)
            .map_err(|_| HelperError)?;
        file.flush().map_err(|_| HelperError)?;
        file.sync_all().map_err(|_| HelperError)?;
        secure_created_file(&request_path)?;
        Ok(Self {
            descriptor: request,
            context,
            #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
            scope,
            _directory: directory,
            request_path,
            listener,
            nonce,
            source_id: source_id.to_owned(),
        })
    }

    /// 返回提升进程唯一允许接收的三个参数。
    #[must_use]
    pub fn arguments(&self) -> [OsString; 3] {
        [
            OsString::from(HELPER_FLAG),
            self.request_path.as_os_str().to_owned(),
            OsString::from(&self.nonce),
        ]
    }

    /// Encode this one-time request for an already authorized Linux helper session.
    ///
    /// The returned JSON frame includes its terminating newline and contains only
    /// the private request-file path and nonce, never the roots or file contents.
    #[cfg(target_os = "linux")]
    pub fn session_request_line(&self) -> Result<Vec<u8>, HelperError> {
        session::request_line(&self.request_path, &self.nonce)
    }

    /// 校验并读取 Helper 通过本机回环连接返回的聚合结果。
    pub fn receive_response(&self) -> Result<ResponseOutcome, HelperError> {
        self.receive_response_with_operation(&self.context)
    }

    pub(super) fn receive_response_with_operation(
        &self,
        operation: &OperationContext,
    ) -> Result<ResponseOutcome, HelperError> {
        self.context.check().map_err(|_| HelperError)?;
        let deadline = self
            .context
            .deadline
            .min(Instant::now() + Duration::from_secs(2));
        let limit = self.context.budget.output_bytes.min(64 * 1024);
        let response = loop {
            self.context.check().map_err(|_| HelperError)?;
            operation.check().map_err(|_| HelperError)?;
            match self.listener.accept() {
                Ok((stream, peer)) if peer.ip().is_loopback() => {
                    stream
                        .set_read_timeout(Some(Duration::from_millis(100)))
                        .map_err(|_| HelperError)?;
                    let mut stream = stream.take(limit as u64 + 1);
                    let mut json = Vec::new();
                    let mut buffer = [0_u8; 4096];
                    loop {
                        self.context.check().map_err(|_| HelperError)?;
                        operation.check().map_err(|_| HelperError)?;
                        if Instant::now() >= deadline {
                            return Err(HelperError);
                        }
                        match stream.read(&mut buffer) {
                            Ok(0) => break,
                            Ok(count) => json.extend_from_slice(&buffer[..count]),
                            Err(error)
                                if matches!(
                                    error.kind(),
                                    std::io::ErrorKind::WouldBlock
                                        | std::io::ErrorKind::TimedOut
                                        | std::io::ErrorKind::Interrupted
                                ) => {}
                            Err(_) => return Err(HelperError),
                        }
                    }
                    break (json.len() <= limit)
                        .then(|| serde_json::from_slice::<HelperResponse>(&json).ok())
                        .flatten();
                }
                Ok(_) => continue,
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(_) => break None,
            }
        };
        response.ok_or(HelperError).and_then(|response| {
            if response.operation != self.context.identity() {
                return Err(HelperError);
            }
            validate_response(response, &self.source_id, &self.nonce)
        })
    }

    /// Withdraw the private control directory so an in-flight helper request stops.
    pub fn cancel(&self) -> Result<(), HelperError> {
        self.context.cancel();
        fs::remove_dir_all(self._directory.path()).map_err(|_| HelperError)
    }

    /// Read an authenticated complete metadata task result from the elevated helper.
    #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
    pub fn receive_scan(
        &self,
        cancellation: &std::sync::atomic::AtomicBool,
    ) -> Result<(spaceprune_plugin_api::HandlerScanResult, bool), HelperError> {
        self.receive_task(cancellation)
    }

    #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
    pub(super) fn receive_task<T: serde::de::DeserializeOwned>(
        &self,
        cancellation: &std::sync::atomic::AtomicBool,
    ) -> Result<(T, bool), HelperError> {
        self.listener
            .set_nonblocking(true)
            .map_err(|_| HelperError)?;
        let deadline = self.context.deadline;
        let (mut stream, peer) = loop {
            if cancellation.load(std::sync::atomic::Ordering::Acquire) {
                return Err(HelperError);
            }
            match self.listener.accept() {
                Ok(connection) => break connection,
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(20))
                }
                Err(_) => return Err(HelperError),
            }
        };
        if !peer.ip().is_loopback() {
            return Err(HelperError);
        }
        stream
            .set_read_timeout(Some(Duration::from_millis(100)))
            .map_err(|_| HelperError)?;
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 16 * 1024];
        loop {
            if cancellation.load(std::sync::atomic::Ordering::Acquire) || Instant::now() >= deadline
            {
                return Err(HelperError);
            }
            match stream.read(&mut buffer) {
                Ok(0) => break,
                Ok(count)
                    if bytes.len() + count
                        <= self.context.budget.output_bytes.min(64 * 1024 * 1024) =>
                {
                    bytes.extend_from_slice(&buffer[..count])
                }
                Ok(_) => return Err(HelperError),
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock
                            | std::io::ErrorKind::TimedOut
                            | std::io::ErrorKind::Interrupted
                    ) => {}
                Err(_) => return Err(HelperError),
            }
        }
        let response: TaskScanResponse<T> =
            serde_json::from_slice(&bytes).map_err(|_| HelperError)?;
        if response.schema_version != REQUEST_SCHEMA_VERSION
            || response.operation != self.context.identity()
            || response.source_id != self.source_id
            || response.nonce != self.nonce
        {
            return Err(HelperError);
        }
        let result = response.result.map_err(|_| HelperError)?;
        Ok((result, response.cancelled))
    }
}

fn encode_request(request: &HelperRequest) -> Result<Vec<u8>, HelperError> {
    let bytes = serde_json::to_vec(request).map_err(|_| HelperError)?;
    if bytes.len() > MAX_REQUEST_BYTES {
        return Err(HelperError);
    }
    Ok(bytes)
}
