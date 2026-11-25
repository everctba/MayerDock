# ADR — Transações de edição (Unit of Work/Memento) e limites

- **Status:** aceito (2026-11-24)
- **Contexto:** o dock tem estado declarativo editável (largura, divisórias, futuro
  workspaces) e recebe ações imperativas de agentes (click/type/capture/dock_pin).
- **Decisão:**
  1. **Unit of Work + Memento** (`PreviewSession`: begin → update → commit/cancel, com
     `dispose` via RAII) **apenas para estado declarativo**. Implementado em
     `src/dock.rs` (`begin_edit`, `commit_edit`, `cancel_edit`, `undo_layout`,
     `redo_layout`, `LayoutSnapshot`) para largura e divisórias. Commit atômico com
     `tmp+rename` (`Config::save`); cancel não grava; undo/redo por commit (pilha 32).
  2. **Ações do agente são Commands**, não transações: `click`, `type_text`,
     `capture_window`, `dock_pin`, `global_*` registram auditoria (`audit.jsonl`) e não
     entram em `begin/commit` — efeitos externos não são reversíveis.
  3. **Observer mínimo:** eventos tipados internos (`session_*`, `lease`, `activity`) e
     `InvalidateRect`/HUD; sem framework pub-sub.
  4. **Documento formal** (workspaces com versão/fingerprint) entra junto com
     `md-d33.3.1`; `config.ini` continua com Memento + `tmp+rename`.
  5. **Rascunho de workspace por agente** (`md-jcf.4`) usa o mesmo ciclo: agente escreve
     `drafts/`, humano aprova no HUD, commit atômico.
- **Consequências:** cancel/undo previsíveis no layout; agentes não ganham “undo”
  falso; menos overengineering que um framework de documentos completo.
- **Referências:** `docs/TASKS.md` T-003/T-004, `src/dock.rs` (PreviewSession),
  `scripts/layout_session_smoke.py`.
