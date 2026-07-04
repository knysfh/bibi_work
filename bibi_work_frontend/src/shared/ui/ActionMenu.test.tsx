import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ActionMenu } from "./ActionMenu";

describe("ActionMenu", () => {
  afterEach(() => cleanup());

  it("closes when clicking outside", async () => {
    const user = userEvent.setup();
    render(
      <div>
        <ActionMenu
          label="More actions"
          items={[{ label: "Disable", onSelect: vi.fn(), danger: true }]}
        />
        <button type="button">Outside</button>
      </div>
    );

    await user.click(screen.getByRole("button", { name: "More actions" }));
    expect(screen.getByRole("menuitem", { name: "Disable" })).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Outside" }));
    expect(screen.queryByRole("menuitem", { name: "Disable" })).not.toBeInTheDocument();
  });

  it("supports keyboard navigation", async () => {
    const user = userEvent.setup();
    const onFirst = vi.fn();
    const onSecond = vi.fn();
    render(
      <ActionMenu
        label="More actions"
        items={[
          { label: "Edit", onSelect: onFirst },
          { label: "Disable", onSelect: onSecond, danger: true }
        ]}
      />
    );

    const trigger = screen.getByRole("button", { name: "More actions" });
    trigger.focus();
    await user.keyboard("{ArrowDown}");
    await waitFor(() => expect(screen.getByRole("menuitem", { name: "Edit" })).toHaveFocus());
    await user.keyboard("{ArrowDown}{Enter}");

    expect(onSecond).toHaveBeenCalledTimes(1);
    expect(onFirst).not.toHaveBeenCalled();
  });
});
