from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path


type MessageValue = (
    None
    | bool
    | int
    | float
    | str
    | bytes
    | list[MessageValue]
    | dict[str, MessageValue]
)
type Message = dict[str, MessageValue]
type ParameterOverrides = dict[str, dict[str, MessageValue]]


@dataclass(frozen=True, slots=True)
class WorkerArguments:
    socket: str | None
    tcp_host: str | None
    tcp_port: int | None
    source: Path
    scene: str
    width: int
    height: int
    fps: str
