import { render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { MessageContentRenderer } from "./MessageContentRenderer";

vi.mock("mermaid", () => ({
  default: {
    initialize: vi.fn(),
    render: vi.fn().mockResolvedValue({
      svg: "<svg><script>alert(1)</script><text>diagram</text></svg>"
    })
  }
}));

describe("MessageContentRenderer", () => {
  it("renders markdown and keeps unsafe file links inert", () => {
    render(
      <MessageContentRenderer
        content={"# Result\n\n[file](file:///etc/passwd)\n\n[site](https://example.com)"}
      />
    );

    expect(screen.getByRole("heading", { name: "Result" })).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "file" })).not.toBeInTheDocument();
    expect(screen.getByRole("link", { name: "site" })).not.toHaveAttribute("target");
  });

  it("routes fenced artifacts through safe previews", async () => {
    render(
      <MessageContentRenderer
        content={[
          "```ts",
          "const answer = 42;",
          "```",
          "```html",
          "<script>window.evil = true</script><p>safe</p>",
          "```",
          "```svg",
          '<svg><script>alert(1)</script><circle cx="5" cy="5" r="5" /></svg>',
          "```",
          "```drawio",
          "<mxfile><diagram>abc</diagram></mxfile>",
          "```",
          "```mermaid",
          "graph TD; A-->B;",
          "```"
        ].join("\n")}
      />
    );

    expect(screen.getByText("const")).toBeInTheDocument();

    const htmlFrame = screen.getByTestId("safe-html-frame") as HTMLIFrameElement;
    expect(htmlFrame.getAttribute("sandbox")).toBe("");
    expect(htmlFrame.getAttribute("srcdoc")).toContain("<p>safe</p>");
    expect(htmlFrame.getAttribute("srcdoc")).not.toContain("<script>");

    expect(screen.getByTestId("safe-svg-preview").innerHTML).not.toContain("<script>");
    expect(screen.getByTestId("drawio-artifact")).toBeInTheDocument();

    await waitFor(() => expect(screen.getByTestId("mermaid-block")).toBeInTheDocument());
    expect(screen.getByTestId("mermaid-block").innerHTML).not.toContain("<script>");
  });
});
