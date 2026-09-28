//! The baseline: each plugin a native child process, spoken to over stdin/stdout.

use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

use crate::library::Scope;
use crate::protocol::{Call, ToGuest, ToHost};
use crate::wasm::DeadlineExceeded;

pub const BIN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../target/release/guest-process");

pub struct ProcessPlugin {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    scope: Scope,
}

impl ProcessPlugin {
    pub async fn spawn(scope: Scope) -> Result<ProcessPlugin> {
        let mut child = Command::new(BIN)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("spawning {BIN}; run build.sh first"))?;
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Ok(ProcessPlugin { child, stdin, stdout, scope })
    }

    pub fn pid(&self) -> u32 {
        self.child.id().unwrap_or(0)
    }

    async fn send(&mut self, msg: &ToGuest) -> Result<()> {
        let body = serde_json::to_vec(msg)?;
        let mut frame = Vec::with_capacity(4 + body.len());
        frame.extend_from_slice(&(body.len() as u32).to_le_bytes());
        frame.extend_from_slice(&body);
        self.stdin.write_all(&frame).await?;
        Ok(())
    }

    async fn recv(&mut self) -> Result<ToHost> {
        let mut len = [0u8; 4];
        self.stdout.read_exact(&mut len).await.context("plugin exited")?;
        let mut body = vec![0u8; u32::from_le_bytes(len) as usize];
        self.stdout.read_exact(&mut body).await?;
        Ok(serde_json::from_slice(&body)?)
    }

    /// One call, answering the plugin's requests for data and state until it is done.
    pub async fn call(&mut self, call: Call) -> Result<Value> {
        self.send(&ToGuest::Call(call)).await?;
        loop {
            match self.recv().await? {
                ToHost::Done { value } => return Ok(value),
                ToHost::Tracks { offset, limit } => {
                    let page = self.scope.page(offset, limit);
                    self.send(&ToGuest::Tracks { page }).await?;
                }
                ToHost::StateGet { key } => {
                    let value = self.scope.state_get(&key);
                    self.send(&ToGuest::State { value }).await?;
                }
                ToHost::StateSet { key, value } => {
                    self.scope.state_set(key, value);
                    self.send(&ToGuest::Ack).await?;
                }
            }
        }
    }

    /// A bounded call: past `budget` the process is killed, since a pipe cannot be interrupted.
    pub async fn call_within(&mut self, call: Call, budget: Duration) -> Result<Value> {
        match tokio::time::timeout(budget, self.call(call)).await {
            Ok(result) => result,
            Err(_) => {
                self.child.kill().await.ok();
                Err(anyhow!(DeadlineExceeded))
            }
        }
    }

    pub async fn noop(&mut self) -> Result<()> {
        self.call(Call::Noop).await.map(|_| ())
    }

    pub async fn echo(&mut self, s: &str) -> Result<String> {
        match self.call(Call::Echo { s: s.to_owned() }).await? {
            Value::String(s) => Ok(s),
            other => bail!("unexpected {other}"),
        }
    }

    pub async fn number(&mut self, call: Call, budget: Duration) -> Result<u64> {
        self.call_within(call, budget).await?.as_u64().ok_or_else(|| anyhow!("not a number"))
    }
}
