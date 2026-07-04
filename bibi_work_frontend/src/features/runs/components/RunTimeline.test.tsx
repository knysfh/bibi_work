import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { PlatformProviders } from "../../../app/providers";
import { createEmptyRunProjection, type RunProjection } from "../domain/run.types";
import { RunTimeline } from "./RunTimeline";

function projectionWithMessages(): RunProjection {
  return {
    ...createEmptyRunProjection(),
    status: "completed",
    lastSeq: 2,
    messages: [
      { id: "user-1", role: "user", content: "你好", status: "completed" },
      {
        id: "assistant-1",
        role: "assistant",
        content: "你好，有什么可以帮你？",
        status: "completed"
      }
    ]
  };
}

function projectionWithToolCall(): RunProjection {
  return {
    ...createEmptyRunProjection(),
    status: "completed",
    lastSeq: 3,
    messages: [{ id: "user-1", role: "user", content: "查看当前目录", status: "completed" }],
    toolCalls: [
      {
        id: "call-1",
        name: "ls",
        status: "completed",
        outputSummary: "2 entries",
        views: [
          {
            kind: "table",
            columns: [
              { key: "path", label: "path", type: "string" },
              { key: "type", label: "type", type: "string" }
            ],
            rowsPreview: [
              { path: "/local/main/", type: "directory" },
              { path: "/local/main/readme.md", type: "file" }
            ]
          }
        ]
      }
    ]
  };
}

function projectionWithInterleavedToolCall(): RunProjection {
  return {
    ...createEmptyRunProjection(),
    status: "completed",
    lastSeq: 5,
    messages: [
      { id: "user-1", role: "user", content: "查看当前目录", status: "completed" },
      {
        id: "assistant-1",
        role: "assistant",
        content: "当前目录下有 readme.md",
        status: "completed"
      }
    ],
    toolCalls: [
      {
        id: "call-1",
        name: "ls",
        status: "completed",
        outputSummary: "1 entry",
        views: []
      }
    ],
    timeline: [
      { kind: "message", id: "user-1" },
      { kind: "tool_call", id: "call-1" },
      { kind: "message", id: "assistant-1" }
    ]
  };
}

describe("RunTimeline", () => {
  const writeText = vi.fn();

  beforeEach(() => {
    writeText.mockReset();
    writeText.mockResolvedValue(undefined);
    if (!window.navigator.clipboard) {
      Object.defineProperty(window.navigator, "clipboard", {
        value: { writeText },
        configurable: true
      });
    }
    vi.spyOn(window.navigator.clipboard, "writeText").mockImplementation(writeText);
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it("copies message content", async () => {
    const user = userEvent.setup();
    render(<RunTimeline projection={projectionWithMessages()} />);

    const copyButton = screen.getAllByRole("button", { name: "复制" })[0];
    await user.click(copyButton);

    await waitFor(() => expect(copyButton.querySelector(".lucide-check")).toBeInTheDocument());
  });

  it("opens a user message for editing", async () => {
    const user = userEvent.setup();
    const onEditMessage = vi.fn();
    render(<RunTimeline projection={projectionWithMessages()} onEditMessage={onEditMessage} />);

    await user.click(screen.getByRole("button", { name: "编辑后重新发送" }));

    expect(onEditMessage).toHaveBeenCalledWith("user-1");
  });

  it("toggles assistant feedback", async () => {
    const user = userEvent.setup();
    render(<RunTimeline projection={projectionWithMessages()} />);

    const like = screen.getByRole("button", { name: "赞" });
    const dislike = screen.getByRole("button", { name: "踩" });
    await user.click(like);
    expect(like).toHaveAttribute("aria-pressed", "true");

    await user.click(dislike);
    expect(like).toHaveAttribute("aria-pressed", "false");
    expect(dislike).toHaveAttribute("aria-pressed", "true");
  });

  it("renders projected tool calls in the chat timeline", () => {
    const { container } = render(
      <PlatformProviders>
        <RunTimeline projection={projectionWithToolCall()} />
      </PlatformProviders>
    );

    expect(container.querySelector("details")).not.toHaveAttribute("open");
    expect(screen.getByText("ls")).toBeInTheDocument();
  });

  it("renders a tool call inside the assistant row before the following assistant text", () => {
    const { container } = render(
      <PlatformProviders>
        <RunTimeline projection={projectionWithInterleavedToolCall()} />
      </PlatformProviders>
    );

    const toolRow = screen.getByText("ls").closest(".tool-call-stack");
    const assistantText = screen.getByText("当前目录下有 readme.md");
    const assistantRow = assistantText.closest(".message-row");
    const toolAssistantRow = toolRow?.closest(".message-row");

    expect(toolAssistantRow).toBe(assistantRow);
    expect(
      toolRow?.compareDocumentPosition(assistantText) ?? Node.DOCUMENT_POSITION_PRECEDING
    ).toBe(Node.DOCUMENT_POSITION_FOLLOWING);
    expect(container.querySelectorAll(".message-assistant")).toHaveLength(1);
  });
});
