"""Parse the user's own chat history exports into prompt/answer pairs. Local only.

Supported inputs (a file or, for Claude Code, a directory of session files):

- ChatGPT data export: the `.zip` from Settings -> Data controls -> Export, or
  the `conversations.json` inside it.
- Claude data export: the `.zip` from Settings -> Privacy -> Export data, or
  the conversations JSON inside it.
- Claude Code sessions: `*.jsonl` files (e.g. under `~/.claude/projects/`).

Nothing here touches the network. Only human/assistant *text* is kept: system
prompts, tool calls/results, thinking blocks and local-command noise are dropped,
because a micro-eval is graded on the question and the answer, not the plumbing.
"""

from __future__ import annotations

import json
import zipfile
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Literal

Source = Literal["chatgpt", "claude", "claude_code"]


@dataclass(frozen=True)
class PromptPair:
    """One user prompt and the assistant answer it actually received."""

    source: Source
    conversation_id: str
    conversation_title: str
    turn_index: int  # index of the user turn within its conversation, 0-based
    created_at: str | None
    model: str | None  # model that produced the answer, when the export records it
    prompt: str
    answer: str


@dataclass
class Conversation:
    source: Source
    id: str
    title: str
    created_at: str | None = None
    turns: list[tuple[str, str, str | None]] = field(default_factory=list)
    """Ordered (role, text, model) turns; role is 'user' or 'assistant'."""

    def pairs(self) -> list[PromptPair]:
        pairs: list[PromptPair] = []
        pending: tuple[int, str] | None = None  # (turn index, prompt text)
        turn_index = -1
        for role, text, model in self.turns:
            if role == "user":
                turn_index += 1
                pending = (turn_index, text)
            elif pending is not None and text.strip():
                pairs.append(
                    PromptPair(
                        source=self.source,
                        conversation_id=self.id,
                        conversation_title=self.title,
                        turn_index=pending[0],
                        created_at=self.created_at,
                        model=model,
                        prompt=pending[1],
                        answer=text,
                    )
                )
                pending = None
        return pairs


@dataclass(frozen=True)
class History:
    conversations: list[Conversation]

    @property
    def pairs(self) -> list[PromptPair]:
        return [pair for conv in self.conversations for pair in conv.pairs()]

    def by_source(self) -> dict[str, int]:
        counts: dict[str, int] = {}
        for conv in self.conversations:
            counts[conv.source] = counts.get(conv.source, 0) + 1
        return counts


# --- shared helpers --------------------------------------------------------------------------


def _text_blocks(content: Any) -> str:
    """Flatten the many 'content' shapes used across exports into plain text."""
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        parts: list[str] = []
        for item in content:
            if isinstance(item, str):
                parts.append(item)
            elif isinstance(item, dict):
                kind = item.get("type")
                if kind in ("text", None) and isinstance(item.get("text"), str):
                    parts.append(item["text"])
                elif kind == "text" and isinstance(item.get("content"), str):
                    parts.append(item["content"])
        return "\n".join(p for p in parts if p)
    if isinstance(content, dict):
        if isinstance(content.get("parts"), list):
            return _text_blocks(content["parts"])
        if isinstance(content.get("text"), str):
            return content["text"]
    return ""


def _read_json_from_zip(path: Path, preferred: tuple[str, ...]) -> Any:
    with zipfile.ZipFile(path) as archive:
        names = [n for n in archive.namelist() if n.lower().endswith(".json")]
        for want in preferred:
            for name in names:
                if name.lower().endswith(want):
                    return json.loads(archive.read(name))
        for name in names:  # fall back to the largest JSON in the archive
            return json.loads(archive.read(name))
    raise ValueError(f"{path}: no JSON file inside this export zip")


def _load_json(path: Path, preferred: tuple[str, ...] = ("conversations.json",)) -> Any:
    if path.suffix.lower() == ".zip":
        return _read_json_from_zip(path, preferred)
    return json.loads(path.read_text(encoding="utf-8"))


# --- ChatGPT export --------------------------------------------------------------------------


def parse_chatgpt(path: Path) -> list[Conversation]:
    """ChatGPT export: a list of conversations, each a tree in `mapping`.

    The visible conversation is the branch ending at `current_node`; follow
    `parent` links back to the root and reverse. Regenerated branches are
    ignored, matching what the user actually kept.
    """
    data = _load_json(path)
    if isinstance(data, dict):
        data = data.get("conversations", [])
    if not isinstance(data, list):
        raise ValueError(f"{path}: not a ChatGPT export (expected a list of conversations)")
    out: list[Conversation] = []
    for raw in data:
        if not isinstance(raw, dict):
            continue
        mapping = raw.get("mapping")
        if not isinstance(mapping, dict):
            continue
        chain: list[dict] = []
        node_id = raw.get("current_node")
        seen: set[str] = set()
        while isinstance(node_id, str) and node_id in mapping and node_id not in seen:
            seen.add(node_id)
            chain.append(mapping[node_id])
            node_id = mapping[node_id].get("parent")
        if not chain:  # old exports without current_node: chronological order
            chain = sorted(
                mapping.values(),
                key=lambda n: (n.get("message") or {}).get("create_time") or 0,
            )
        else:
            chain.reverse()
        conv = Conversation(
            source="chatgpt",
            id=str(raw.get("conversation_id") or raw.get("id") or f"chatgpt-{len(out)}"),
            title=str(raw.get("title") or ""),
            created_at=None if raw.get("create_time") is None else str(raw["create_time"]),
        )
        for node in chain:
            message = node.get("message")
            if not isinstance(message, dict):
                continue
            author = (message.get("author") or {}).get("role")
            if author not in ("user", "assistant"):
                continue
            if (message.get("metadata") or {}).get("is_visually_hidden_from_conversation"):
                continue
            text = _text_blocks(message.get("content")).strip()
            model = (message.get("metadata") or {}).get("model_slug")
            if text:
                conv.turns.append((author, text, model if isinstance(model, str) else None))
        if conv.pairs():
            out.append(conv)
    return out


# --- Claude export ---------------------------------------------------------------------------


def parse_claude(path: Path) -> list[Conversation]:
    """Claude.ai export: a list of conversations with `chat_messages`.

    Each message has `sender` ('human'/'assistant') and either a `text` string
    or a `content` list of blocks. Attachments and tool blocks are skipped.
    """
    data = _load_json(path)
    if isinstance(data, dict):
        data = data.get("conversations", [])
    if not isinstance(data, list):
        raise ValueError(f"{path}: not a Claude export (expected a list of conversations)")
    out: list[Conversation] = []
    for raw in data:
        if not isinstance(raw, dict):
            continue
        conv = Conversation(
            source="claude",
            id=str(raw.get("uuid") or raw.get("id") or f"claude-{len(out)}"),
            title=str(raw.get("name") or raw.get("title") or ""),
            created_at=None if raw.get("created_at") is None else str(raw["created_at"]),
        )
        for message in raw.get("chat_messages") or []:
            if not isinstance(message, dict):
                continue
            role = {"human": "user", "user": "user", "assistant": "assistant"}.get(
                str(message.get("sender") or message.get("role") or "")
            )
            if role is None:
                continue
            text = _text_blocks(message.get("content")).strip()
            if not text and isinstance(message.get("text"), str):
                text = message["text"].strip()
            model = message.get("model")
            if text:
                conv.turns.append((role, text, model if isinstance(model, str) else None))
        if conv.pairs():
            out.append(conv)
    return out


# --- Claude Code sessions --------------------------------------------------------------------

_CC_NOISE_PREFIXES = ("<command-name>", "<local-command-", "Caveat:")


def parse_claude_code(path: Path) -> list[Conversation]:
    """Claude Code `*.jsonl` session transcripts, one conversation per file.

    Keeps real user text and assistant text. Tool results, tool calls, thinking
    blocks, meta events and slash-command wrappers are dropped.
    """
    files = sorted(path.rglob("*.jsonl")) if path.is_dir() else [path]
    out: list[Conversation] = []
    for file in files:
        conv = Conversation(source="claude_code", id=file.stem, title="", created_at=None)
        for line in file.read_text(encoding="utf-8", errors="replace").splitlines():
            # Fast path, mirrored in the Rust parser: tool-result lines are most
            # of the bytes in real session files and are dropped below anyway,
            # so skip their full JSON parse.
            if (
                not line.strip()
                or '"tool_result"' in line
                and ('"type":"tool_result"' in line or '"type": "tool_result"' in line)
            ):
                continue
            if "user" not in line and "assistant" not in line:
                continue
            try:
                event = json.loads(line)
            except json.JSONDecodeError:
                continue
            if not isinstance(event, dict) or event.get("isMeta"):
                continue
            kind = event.get("type")
            if kind not in ("user", "assistant"):
                continue
            message = event.get("message") or {}
            raw_content = message.get("content") if isinstance(message, dict) else None
            if isinstance(raw_content, list) and any(
                isinstance(b, dict) and b.get("type") == "tool_result" for b in raw_content
            ):
                continue  # tool output is not a human prompt
            text = _text_blocks(raw_content).strip()
            if not text or text.startswith(_CC_NOISE_PREFIXES):
                continue
            if conv.created_at is None and event.get("timestamp"):
                conv.created_at = str(event["timestamp"])
            model = message.get("model") if isinstance(message, dict) else None
            conv.turns.append((kind, text, model if isinstance(model, str) else None))
        if conv.pairs():
            out.append(conv)
    return out


# --- entry point -----------------------------------------------------------------------------


def load_history(paths: list[Path], source: Source | Literal["auto"] = "auto") -> History:
    """Load one or more exports, auto-detecting each one's format by content."""
    conversations: list[Conversation] = []
    for path in paths:
        if not path.exists():
            raise FileNotFoundError(f"no such file or directory: {path}")
        if source != "auto":
            parser = {
                "chatgpt": parse_chatgpt,
                "claude": parse_claude,
                "claude_code": parse_claude_code,
            }[source]
            conversations.extend(parser(path))
            continue
        if path.is_dir() or path.suffix.lower() == ".jsonl":
            conversations.extend(parse_claude_code(path))
            continue
        probe = _load_json(path)
        conversations.extend(
            parse_chatgpt(path) if _looks_like_chatgpt(probe) else parse_claude(path)
        )
    return History(conversations)


def _looks_like_chatgpt(data: Any) -> bool:
    items = data.get("conversations", []) if isinstance(data, dict) else data
    if not isinstance(items, list) or not items:
        return False
    first = items[0]
    return isinstance(first, dict) and "mapping" in first
