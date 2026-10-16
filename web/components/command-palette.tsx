"use client";
import { useEffect, useMemo, useRef, useState } from "react";
import {
  ArrowDownToLine,
  CornerDownLeft,
  LayoutDashboard,
  LayoutList,
  LogOut,
  Map,
  Moon,
  Plus,
  Radar,
  Search,
  Server,
  Settings2,
  UserRound,
  Sun,
} from "lucide-react";
import { Dialog, DialogContent, DialogTitle } from "@/components/ui/dialog";
import { StatusPill } from "@/components/status-pill";
import { matchesSearch } from "@/lib/fleet";
import type { Host } from "@/lib/protocol";

export type Command = {
  id: string;
  label: string;
  hint?: string;
  icon: React.ComponentType<{ size?: number }>;
  run: () => void;
};

const ICONS = {
  overview: LayoutDashboard,
  map: Map,
  hosts: LayoutList,
  discover: Radar,
  network: Plus,
  agent: ArrowDownToLine,
  settings: Settings2,
  theme: Sun,
  themeDark: Moon,
  logout: LogOut,
  account: UserRound,
};

export function CommandPalette({
  open,
  onOpenChange,
  hosts,
  commands,
  onSelectHost,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  hosts: Host[];
  commands: Command[];
  onSelectHost: (host: Host) => void;
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        className="palette sm:max-w-lg"
        showCloseButton={false}
        aria-label="Command palette"
      >
        <DialogTitle className="sr-only">Search and commands</DialogTitle>
        <PaletteBody
          hosts={hosts}
          commands={commands}
          onSelectHost={onSelectHost}
          close={() => onOpenChange(false)}
        />
      </DialogContent>
    </Dialog>
  );
}

/** Mounted fresh on every open, so the query and cursor always start clean. */
function PaletteBody({
  hosts,
  commands,
  onSelectHost,
  close,
}: {
  hosts: Host[];
  commands: Command[];
  onSelectHost: (host: Host) => void;
  close: () => void;
}) {
  const [query, setQuery] = useState("");
  const needle = query.trim().toLowerCase();
  // The cursor belongs to the query it was moved under; a new query starts at the top.
  const [cursorState, setCursorState] = useState({ needle: "", index: 0 });
  const cursor = cursorState.needle === needle ? cursorState.index : 0;
  const setCursor = (update: number | ((current: number) => number)) =>
    setCursorState({
      needle,
      index: typeof update === "function" ? update(cursor) : update,
    });
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const matchedHosts = useMemo(
    () =>
      hosts
        .filter((host) => matchesSearch(host, needle))
        .sort((a, b) => a.name.localeCompare(b.name))
        .slice(0, needle ? 12 : 6),
    [hosts, needle],
  );
  const matchedCommands = useMemo(
    () =>
      commands.filter(
        (command) =>
          !needle ||
          `${command.label} ${command.hint ?? ""}`
            .toLowerCase()
            .includes(needle),
      ),
    [commands, needle],
  );
  const items = useMemo(
    () => [
      ...matchedHosts.map((host) => ({
        key: `host:${host.id}`,
        run: () => onSelectHost(host),
      })),
      ...matchedCommands.map((command) => ({
        key: `command:${command.id}`,
        run: command.run,
      })),
    ],
    [matchedHosts, matchedCommands, onSelectHost],
  );
  useEffect(() => {
    const element = listRef.current?.querySelector(
      '[aria-selected="true"]',
    ) as HTMLElement | null;
    element?.scrollIntoView({ block: "nearest" });
  }, [cursor]);
  const choose = (index: number) => {
    const item = items[index];
    if (!item) return;
    close();
    item.run();
  };
  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setCursor((c) => Math.min(items.length - 1, c + 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setCursor((c) => Math.max(0, c - 1));
    } else if (e.key === "Enter") {
      e.preventDefault();
      choose(cursor);
    }
  };
  let index = -1;
  return (
    <>
      <div className="palette-input">
        <Search size={17} />
        <input
          ref={inputRef}
          autoFocus
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={onKeyDown}
          placeholder="Jump to a host or run a command…"
          aria-label="Command palette search"
          role="combobox"
          aria-expanded="true"
          aria-controls="palette-results"
          aria-autocomplete="list"
          autoComplete="off"
          spellCheck={false}
        />
      </div>
      <div
        className="palette-list"
        id="palette-results"
        role="listbox"
        ref={listRef}
      >
        {!items.length && (
          <p className="palette-empty">Nothing matches “{query}”.</p>
        )}
        {matchedHosts.length > 0 && <div className="palette-group">Hosts</div>}
        {matchedHosts.map((host) => {
          const i = ++index;
          return (
            <button
              key={host.id}
              type="button"
              role="option"
              className="palette-item"
              aria-selected={cursor === i}
              onMouseEnter={() => setCursor(i)}
              onClick={() => choose(i)}
            >
              <Server />
              <span className="truncate">{host.name}</span>
              <StatusPill host={host} />
              <span className="meta">{host.addresses[0] ?? ""}</span>
            </button>
          );
        })}
        {matchedCommands.length > 0 && (
          <div className="palette-group">Commands</div>
        )}
        {matchedCommands.map((command) => {
          const i = ++index;
          const Icon = command.icon;
          return (
            <button
              key={command.id}
              type="button"
              role="option"
              className="palette-item"
              aria-selected={cursor === i}
              onMouseEnter={() => setCursor(i)}
              onClick={() => choose(i)}
            >
              <Icon />
              <span className="truncate">{command.label}</span>
              {command.hint && <span className="meta">{command.hint}</span>}
            </button>
          );
        })}
      </div>
      <div className="palette-footer">
        <span>
          <kbd className="key">↑</kbd>
          <kbd className="key">↓</kbd> navigate
        </span>
        <span>
          <kbd className="key">
            <CornerDownLeft size={10} />
          </kbd>{" "}
          open
        </span>
        <span>
          <kbd className="key">esc</kbd> close
        </span>
      </div>
    </>
  );
}

export const commandIcons = ICONS;
