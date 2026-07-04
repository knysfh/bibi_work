import type { RunEvent } from "../../../shared/contracts/platform";
import type { ToolResultView } from "./tool-result-view.types";

export interface TimelineMessage {
  id: string;
  role: "user" | "assistant" | "system";
  content: string;
  status: "streaming" | "completed";
}

export interface ToolCallProjection {
  id: string;
  name: string;
  status: "running" | "completed" | "failed";
  riskLevel?: string;
  inputSummary?: string;
  outputSummary?: string;
  errorSummary?: string;
  views: ToolResultView[];
}

export type TimelineProjectionItem =
  | { kind: "message"; id: string }
  | { kind: "tool_call"; id: string };

export interface TaskProjection {
  id: string;
  title: string;
  status: string;
  summary?: string;
}

export interface SubagentProjection {
  id: string;
  name: string;
  status: string;
  summary?: string;
}

export interface FileChangeProjection {
  path: string;
  revision?: number;
  contentHash?: string;
  reason?: string;
}

export interface ApprovalProjection {
  id: string;
  status: string;
  riskLevel?: string;
  toolName?: string;
}

export interface ConversationProjection {
  status: string;
  activeRunId?: string;
  lastSeq: number;
  messages: TimelineMessage[];
}

export interface RunObservationProjection {
  runEvents: RunEvent[];
  timeline: TimelineProjectionItem[];
  toolCalls: ToolCallProjection[];
  tasks: TaskProjection[];
  subagents: SubagentProjection[];
  files: FileChangeProjection[];
  approvals: ApprovalProjection[];
  memoryCandidateCount: number;
}

export interface RunProjection extends ConversationProjection, RunObservationProjection {
  seenEventKeys: Set<string>;
  events: RunEvent[];
}

export function createEmptyRunProjection(): RunProjection {
  return {
    status: "idle",
    activeRunId: undefined,
    lastSeq: 0,
    seenEventKeys: new Set(),
    messages: [],
    runEvents: [],
    timeline: [],
    toolCalls: [],
    tasks: [],
    subagents: [],
    files: [],
    approvals: [],
    memoryCandidateCount: 0,
    events: []
  };
}
