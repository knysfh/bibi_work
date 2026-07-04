import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { Panel, ResourceList, SegmentedControl } from "./index";

describe("layout primitives", () => {
  afterEach(() => cleanup());

  it("renders panel title, subtitle, actions, and content", () => {
    render(
      <Panel title="Devices" subtitle="6 devices" actions={<button type="button">Refresh</button>}>
        <p>Panel body</p>
      </Panel>
    );

    expect(screen.getByText("Devices")).toBeInTheDocument();
    expect(screen.getByText("6 devices")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Refresh" })).toBeInTheDocument();
    expect(screen.getByText("Panel body")).toBeInTheDocument();
  });

  it("keeps resource list base class while accepting page-specific classes", () => {
    render(
      <ResourceList className="memory-list">
        <button type="button">Memory item</button>
      </ResourceList>
    );

    const list = screen.getByRole("button", { name: "Memory item" }).parentElement;
    expect(list).toHaveClass("resource-list");
    expect(list).toHaveClass("memory-list");
  });

  it("reports segmented control changes", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(
      <SegmentedControl
        label="Status"
        items={[
          { id: "active", label: "Active" },
          { id: "archived", label: "Archived" }
        ]}
        active="active"
        onChange={onChange}
      />
    );

    expect(screen.getByRole("tab", { name: "Active" })).toHaveAttribute("aria-selected", "true");
    await user.click(screen.getByRole("tab", { name: "Archived" }));

    expect(onChange).toHaveBeenCalledWith("archived");
  });
});
