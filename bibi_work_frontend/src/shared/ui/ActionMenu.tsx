import { MoreHorizontal } from "lucide-react";
import { useEffect, useRef, useState, type KeyboardEvent, type ReactNode } from "react";

export interface ActionMenuItem {
  label: string;
  icon?: ReactNode;
  disabled?: boolean;
  danger?: boolean;
  onSelect: () => void;
}

export function ActionMenu({ label, items }: { label: string; items: ActionMenuItem[] }) {
  const visibleItems = items.filter(Boolean);
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement | null>(null);
  const triggerRef = useRef<HTMLButtonElement | null>(null);
  const itemRefs = useRef<Array<HTMLButtonElement | null>>([]);

  useEffect(() => {
    if (!open) {
      return;
    }
    function handlePointerDown(event: PointerEvent) {
      if (rootRef.current && !rootRef.current.contains(event.target as Node)) {
        setOpen(false);
      }
    }
    document.addEventListener("pointerdown", handlePointerDown);
    return () => document.removeEventListener("pointerdown", handlePointerDown);
  }, [open]);

  if (!visibleItems.length) {
    return null;
  }

  function enabledIndexes() {
    return visibleItems.flatMap((item, index) => (item.disabled ? [] : [index]));
  }

  function focusItem(index: number) {
    itemRefs.current[index]?.focus();
  }

  function focusFirst() {
    const [first] = enabledIndexes();
    if (first !== undefined) {
      window.requestAnimationFrame(() => focusItem(first));
    }
  }

  function focusLast() {
    const indexes = enabledIndexes();
    const last = indexes[indexes.length - 1];
    if (last !== undefined) {
      window.requestAnimationFrame(() => focusItem(last));
    }
  }

  function moveFocus(direction: 1 | -1) {
    const indexes = enabledIndexes();
    if (!indexes.length) {
      return;
    }
    const activeIndex = itemRefs.current.findIndex((item) => item === document.activeElement);
    const currentPosition = indexes.indexOf(activeIndex);
    const nextPosition =
      currentPosition === -1
        ? direction === 1
          ? 0
          : indexes.length - 1
        : (currentPosition + direction + indexes.length) % indexes.length;
    focusItem(indexes[nextPosition]);
  }

  function handleTriggerKeyDown(event: KeyboardEvent<HTMLButtonElement>) {
    if (event.key === "ArrowDown") {
      event.preventDefault();
      setOpen(true);
      focusFirst();
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      setOpen(true);
      focusLast();
    }
  }

  function handleMenuKeyDown(event: KeyboardEvent<HTMLDivElement>) {
    if (event.key === "Escape") {
      event.preventDefault();
      setOpen(false);
      triggerRef.current?.focus();
    } else if (event.key === "ArrowDown") {
      event.preventDefault();
      moveFocus(1);
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      moveFocus(-1);
    } else if (event.key === "Home") {
      event.preventDefault();
      focusFirst();
    } else if (event.key === "End") {
      event.preventDefault();
      focusLast();
    }
  }

  return (
    <div className="action-menu" ref={rootRef}>
      <button
        ref={triggerRef}
        type="button"
        className="action-menu-trigger"
        aria-label={label}
        title={label}
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen((current) => !current)}
        onKeyDown={handleTriggerKeyDown}
      >
        <MoreHorizontal size={16} />
      </button>
      {open ? (
        <div className="action-menu-popover" role="menu" onKeyDown={handleMenuKeyDown}>
          {visibleItems.map((item, index) => (
            <button
              key={item.label}
              ref={(element) => {
                itemRefs.current[index] = element;
              }}
              type="button"
              role="menuitem"
              className={item.danger ? "danger" : ""}
              disabled={item.disabled}
              onClick={() => {
                setOpen(false);
                item.onSelect();
              }}
            >
              {item.icon}
              <span>{item.label}</span>
            </button>
          ))}
        </div>
      ) : null}
    </div>
  );
}
