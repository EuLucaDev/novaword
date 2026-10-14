//! Colaboração em tempo real (Nova Live) no motor do Nova Word.
//!
//! - Comandos que alteram o documento viram [`LiveChange::Op`] com a seleção (cursor) de quem
//!   digitou, para o colega rodar o mesmo comando no mesmo ponto do texto.
//! - Qualquer outra mudança (desfazer, refazer, arrastes) vira [`LiveChange::Replace`]: a revisão
//!   do documento mudou sem um comando replicável.
//! - Cursor, seleção, vista e histórico de desfazer continuam de cada pessoa.

use novaword_doc::Document;
use serde_json::{Value, json};

use crate::CmdError;
use crate::session::{Selection, Session};

#[derive(Clone, Debug, PartialEq)]
pub enum LiveChange {
    Op { cmd: String, params: Value, ctx: Value },
    Replace,
}

#[derive(Default)]
pub struct LiveState {
    pub on: bool,
    pub outbox: Vec<LiveChange>,
    pub(crate) applying: bool,
    /// Revisão já informada à sessão.
    pub(crate) seen: u64,
    /// Comandos rodando agora (comandos que chamam comandos): só o de fora é replicado.
    pub(crate) depth: u32,
}

fn err(e: impl std::fmt::Display) -> CmdError {
    CmdError::Failed(format!("colaboração: {e}"))
}

/// Impressão digital estável (FNV-1a 64).
pub fn fnv64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

// Only audited document edits may execute on a peer. Other local edits travel as data.
fn remote_command_allowed(cmd: &str) -> bool {
    matches!(cmd, "text.insert" | "text.deleteBackward" | "format.bold" | "format.italic")
}

impl Session {
    pub fn live_start(&mut self) -> Result<(), CmdError> {
        self.live = LiveState { on: true, seen: self.revision(), ..LiveState::default() };
        Ok(())
    }

    pub fn live_stop(&mut self) {
        self.live = LiveState::default();
    }

    pub fn live_active(&self) -> bool {
        self.live.on
    }

    pub fn live_take(&mut self) -> Vec<LiveChange> {
        if self.live.on && self.revision() != self.live.seen {
            self.live.seen = self.revision();
            self.live.outbox.push(LiveChange::Replace);
        }
        std::mem::take(&mut self.live.outbox)
    }

    pub fn live_busy(&self) -> bool {
        false
    }

    pub fn live_snapshot(&self) -> Result<Vec<u8>, CmdError> {
        serde_json::to_vec(&self.doc).map_err(err)
    }

    pub fn live_digest(&self) -> Result<u64, CmdError> {
        Ok(fnv64(&self.live_snapshot()?))
    }

    pub fn live_load(&mut self, bytes: &[u8]) -> Result<(), CmdError> {
        let doc: Document = serde_json::from_slice(bytes).map_err(err)?;
        self.doc = doc;
        self.touch();
        self.clamp_selection();
        self.forget_undo();
        self.live.seen = self.revision();
        Ok(())
    }

    /// O desfazer guarda fotos do documento: depois de uma alteração de colega, desfazer o passo
    /// anterior apagaria o trabalho dele. O histórico local recomeça daqui.
    fn forget_undo(&mut self) {
        let (history, evicted, _, _) = self.undo_stacks();
        self.restore_undo_stacks((Vec::new(), evicted + history.len() as u64, Vec::new(), false));
    }

    /// Roda o comando de um colega com o cursor dele; desfazer e cursor locais ficam como estavam.
    pub fn live_apply(&mut self, cmd: &str, params: &Value, ctx: &Value) -> Result<(), CmdError> {
        if !remote_command_allowed(cmd) {
            return Err(err("comando não permitido na colaboração"));
        }
        let local = self.sel.clone();
        if let Some(sel) = ctx.get("selection").and_then(|s| serde_json::from_value::<Selection>(s.clone()).ok()) {
            self.sel = sel;
            self.clamp_selection();
        }
        self.live.applying = true;
        let r = self.run(cmd, params);
        self.live.applying = false;
        self.forget_undo();
        self.sel = local;
        self.clamp_selection();
        self.live.seen = self.revision();
        r.map(|_| ())
    }

    /// Chamado por `run` depois de cada comando de fora.
    pub(crate) fn live_record(&mut self, id: &str, params: &Value, mutates: bool, ctx: Value) {
        if self.live.applying || !self.live.on || self.revision() == self.live.seen {
            return;
        }
        if mutates && remote_command_allowed(id) && !matches!(id, "edit.undo" | "edit.redo" | "edit.undoTo" | "edit.repeat") {
            self.live.outbox.push(LiveChange::Op { cmd: id.to_string(), params: params.clone(), ctx });
            self.live.seen = self.revision();
        }
    }
}

pub(crate) fn context(selection: &Selection) -> Value {
    json!({ "selection": selection })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    fn session() -> Session {
        let mut s = Session::new(Document::default());
        s.run("file.new", &json!({})).unwrap();
        s
    }

    #[test]
    fn remote_commands_cannot_open_export_or_execute_code() {
        let (_, mut b) = pair();
        let before = b.live_digest().unwrap();
        for command in ["file.open", "file.saveAs", "file.export", "session.open", "script.run", "app.quit", "lisp.eval"] {
            assert!(b.live_apply(command, &serde_json::json!({"path": "/tmp/not-authorized"}), &serde_json::json!({})).is_err());
            assert_eq!(before, b.live_digest().unwrap());
        }
    }

    fn pair() -> (Session, Session) {
        let mut a = session();
        a.live_start().unwrap();
        let mut b = session();
        b.live_start().unwrap();
        let snap = a.live_snapshot().unwrap();
        b.live_load(&snap).unwrap();
        (a, b)
    }

    fn sync(a: &mut Session, b: &mut Session) {
        for change in a.live_take() {
            match change {
                LiveChange::Op { cmd, params, ctx } => b.live_apply(&cmd, &params, &ctx).unwrap(),
                LiveChange::Replace => {
                    let snap = a.live_snapshot().unwrap();
                    b.live_load(&snap).unwrap();
                }
            }
        }
        assert!(b.live_take().is_empty(), "o que veio de A não volta para A");
    }

    #[test]
    fn typing_replicates_at_the_authors_caret_and_undo_travels_as_a_document() {
        let (mut a, mut b) = pair();
        a.run("text.insert", &json!({"text": "Relatório de vendas"})).unwrap();
        sync(&mut a, &mut b);
        assert_eq!(a.live_digest().unwrap(), b.live_digest().unwrap());

        // B digita em outro ponto sem perder o próprio cursor.
        b.run("text.insert", &json!({"text": " — outubro"})).unwrap();
        sync(&mut b, &mut a);
        assert_eq!(a.live_digest().unwrap(), b.live_digest().unwrap());

        a.run("edit.undo", &json!({})).unwrap();
        sync(&mut a, &mut b);
        assert_eq!(a.live_digest().unwrap(), b.live_digest().unwrap());
    }
}
