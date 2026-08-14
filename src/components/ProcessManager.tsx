import React, { useState, useEffect, useMemo, useRef } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { useVirtualizer } from "@tanstack/react-virtual";
import { useInterval } from "ahooks";
import { Splitter } from "antd";
import {
  Box, Paper, Typography, Avatar, Switch, TextField,
  Divider, Table, TableCell, TableHead, TableRow, Chip,
  CircularProgress, IconButton, Tooltip,
} from "@mui/material";
import WindowIcon from "@mui/icons-material/Window";
import SearchIcon from "@mui/icons-material/Search";
import MemoryIcon from "@mui/icons-material/Memory";
import ErrorOutlineIcon from "@mui/icons-material/ErrorOutlineOutlined";
import SpeedPanel from "./SpeedPanel";
import ProcessDetail from "./ProcessDetail";
import { useSettings, useSpeed } from "../hooks/useSettings";
import { useSnackbar } from "../contexts/SnackbarContext";

// ── Types & constants ────────────────────────────────────────────────────

interface ProcessInfo {
  pid: number;
  name: string;
  arch: string;
  window_title: string | null;
  memory_kb: number;
  exe_path: string | null;
  admin: boolean;
}

type SpeedPhase = "initializing" | "enabled" | "disabled" | "failed";

interface SpeedState {
  injected: boolean;
  arch: string;
  phase: SpeedPhase;
  error?: string;
}

const ROW_H = 42;
const COL = { pid: 72, check: 60 } as const;

function bridgeErrorMessage(error: unknown): string {
  if (typeof error === "string") return error;
  if (error && typeof error === "object" && "message" in error) {
    const message = (error as { message?: unknown }).message;
    if (typeof message === "string") return message;
  }
  try {
    const serialized = JSON.stringify(error);
    return serialized || String(error);
  } catch {
    return String(error);
  }
}

const INJECTION_PENDING_MARKER = "INJECTION_PENDING:";
const BRIDGE_OUTCOME_UNKNOWN_MARKER = "BRIDGE_OUTCOME_UNKNOWN:";

function bridgeOutcomeNeedsStatus(error: string): boolean {
  return error.includes(INJECTION_PENDING_MARKER) || error.includes(BRIDGE_OUTCOME_UNKNOWN_MARKER);
}

async function invokeBridgeCommand(command: string, args: Record<string, unknown>): Promise<string | null> {
  try {
    await invoke(command, args);
    return null;
  } catch (error) {
    const detail = bridgeErrorMessage(error);
    console.error(`[toggle] ${command} failed:`, detail);
    return detail;
  }
}

function ProcessIcon({ pid, icons }: { pid: number; icons: Record<number, string> }) {
  const src = icons[pid];
  if (src) return <Avatar src={src} variant="rounded" sx={{ width: 22, height: 22, flexShrink: 0, borderRadius: 0.5 }} />;
  return (
    <Avatar variant="rounded" sx={{ width: 22, height: 22, flexShrink: 0, bgcolor: "transparent", borderRadius: 0.5 }}>
      <WindowIcon sx={{ fontSize: 15, color: "text.disabled" }} />
    </Avatar>
  );
}

// ── Memoized process table (isolated from speed state) ───────────────────

const ProcessRow = React.memo(function ProcessRow({
  p, speedState, icons, start, selected, onToggle, onSelect,
}: {
  p: ProcessInfo; speedState?: SpeedState; icons: Record<number, string>; start: number; selected: boolean;
  onToggle: (pid: number, arch: string) => void;
  onSelect: (pid: number) => void;
}) {
  const on = speedState?.phase === "enabled";
  const initializing = speedState?.phase === "initializing";
  const failed = speedState?.phase === "failed";

  return (
    <Box
      onClick={() => onSelect(p.pid)}
      sx={{
        display: "grid", gridTemplateColumns: `${COL.pid}px 1fr ${COL.check}px`,
        position: "absolute", top: 0, left: 0, right: 0, height: ROW_H, transform: `translateY(${start}px)`,
        alignItems: "center", borderBottom: 1, borderColor: "divider", cursor: "pointer",
        bgcolor: selected ? "rgba(92,107,192,0.12)" : on ? "action.selected" : "transparent",
        "&:hover": { bgcolor: selected ? "rgba(92,107,192,0.18)" : on ? "action.selected" : "action.hover" },
      }}
    >
      <Typography variant="body2" color="text.secondary">{p.pid}</Typography>
      <Box sx={{ display: "flex", alignItems: "center", gap: 1.2, minWidth: 0 }}>
        <ProcessIcon pid={p.pid} icons={icons} />
        <Box sx={{ minWidth: 0 }}>
          <Typography variant="body2" noWrap sx={{ fontWeight: 500 }}>{p.name}</Typography>
          {p.window_title && <Typography variant="caption" noWrap sx={{ color: "text.disabled", display: "block", lineHeight: 1.2 }}>{p.window_title}</Typography>}
        </Box>
      </Box>
      <Box sx={{ height: 32, display: "flex", alignItems: "center", justifyContent: "center" }}>
        {initializing ? (
          <CircularProgress size={18} thickness={5} />
        ) : failed ? (
          <Tooltip title={speedState?.error ?? ""}>
            <IconButton
              size="small"
              color="error"
              onClick={event => { event.stopPropagation(); onToggle(p.pid, p.arch); }}
            >
              <ErrorOutlineIcon fontSize="small" />
            </IconButton>
          </Tooltip>
        ) : (
          <Switch size="small" checked={on} onChange={() => onToggle(p.pid, p.arch)} />
        )}
      </Box>
    </Box>
  );
}, (prev, next) =>
  prev.p.pid === next.p.pid &&
  prev.p.name === next.p.name &&
  prev.p.arch === next.p.arch &&
  prev.speedState?.phase === next.speedState?.phase &&
  prev.speedState?.error === next.speedState?.error &&
  prev.start === next.start &&
  prev.selected === next.selected
);

const ProcessTable = function ProcessTable({
  processes, filtered, search, onSearch, icons, speedStates, selectedPid, onToggle, onSelect,
}: {
  processes: ProcessInfo[];
  filtered: ProcessInfo[];
  search: string;
  onSearch: (v: string) => void;
  icons: Record<number, string>;
  speedStates: Map<number, SpeedState>;
  selectedPid: number | null;
  onToggle: (pid: number, arch: string) => void;
  onSelect: (pid: number) => void;
}) {
  const { t } = useTranslation();
  const scrollRef = useRef<HTMLDivElement>(null);
  const vz = useVirtualizer({ count: filtered.length, getScrollElement: () => scrollRef.current!, estimateSize: () => ROW_H, overscan: 12 });

  return (
    <Paper elevation={0} sx={{ height: "100%", bgcolor: "background.paper", border: 1, borderColor: "divider", display: "flex", flexDirection: "column", overflow: "hidden" }}>
      <Box sx={{ px: 2, pt: 1.5, pb: 0.5, display: "flex", alignItems: "center" }}>
        <MemoryIcon sx={{ color: "primary.main", fontSize: 18, mr: 1 }} />
        <Typography variant="caption" sx={{ fontWeight: 600, textTransform: "uppercase", letterSpacing: 1, color: "text.secondary" }}>{t("process.title")}</Typography>
        <Typography variant="caption" sx={{ ml: 1, fontWeight: 600, color: "primary.main" }}>{filtered.length} / {processes.length}</Typography>
      </Box>

      <Box sx={{ px: 2, pb: 1, display: "flex", flexDirection: "column", gap: 0.75 }}>
        <TextField placeholder={t("process.search")} variant="outlined" size="small" fullWidth value={search} onChange={e => onSearch(e.target.value)} />
        <Box sx={{ display: "flex", alignItems: "center", gap: 0.75 }}>
          <Chip
            label="Asura"
            size="small"
            color={search.trim().toLowerCase() === "asura" ? "primary" : "default"}
            variant={search.trim().toLowerCase() === "asura" ? "filled" : "outlined"}
            onClick={() => onSearch(search.trim().toLowerCase() === "asura" ? "" : "Asura")}
            sx={{ cursor: "pointer" }}
          />
        </Box>
      </Box>
      <Divider />

      <Box sx={{ px: 2, flex: 1, display: "flex", flexDirection: "column", overflow: "hidden" }}>
        <Table size="small" sx={{ tableLayout: "fixed", flexShrink: 0 }}>
          <colgroup><col width={COL.pid} /><col /><col width={COL.check} /></colgroup>
          <TableHead><TableRow>
            <TableCell>{t("process.pid")}</TableCell><TableCell>{t("process.name")}</TableCell><TableCell align="center">{t("process.enable")}</TableCell>
          </TableRow></TableHead>
        </Table>

        <Box ref={scrollRef} sx={{ flex: 1, overflow: "auto", position: "relative" }}>
          <div style={{ height: vz.getTotalSize(), width: 1 }} />
          {vz.getVirtualItems().map(vr => (
            <ProcessRow key={filtered[vr.index].pid} p={filtered[vr.index]} speedState={speedStates.get(filtered[vr.index].pid)} icons={icons} start={vr.start} selected={selectedPid === filtered[vr.index].pid} onToggle={onToggle} onSelect={onSelect} />
          ))}
          {filtered.length === 0 && (
            <Box sx={{ position: "absolute", inset: 0, display: "flex", alignItems: "center", justifyContent: "center", flexDirection: "column", gap: 1 }}>
              <SearchIcon sx={{ color: "text.disabled", fontSize: 36 }} />
              <Typography variant="body2" color="text.disabled">{search ? t("process.noResults") : t("process.loading")}</Typography>
            </Box>
          )}
        </Box>
      </Box>
    </Paper>
  );
}

// ── Component ────────────────────────────────────────────────────────────

type BridgeStatus =
  | { state: "enabled" | "disabled" | "initializing" | "not_injected" }
  | { state: "failed"; detail: string };

function reconcileBridgeStatus(
  states: Map<number, SpeedState>,
  pid: number,
  arch: string,
  status: BridgeStatus,
): Map<number, SpeedState> {
  const current = states.get(pid);
  if (status.state === "not_injected") {
    if (!current) return states;
    const next = new Map(states);
    next.delete(pid);
    return next;
  }

  if (status.state === "initializing") {
    if (current?.phase === "initializing" && current.arch === arch) return states;
    const next = new Map(states);
    next.set(pid, { injected: false, arch, phase: "initializing" });
    return next;
  }

  if (status.state === "failed") {
    if (current?.phase === "failed" && current.error === status.detail && current.arch === arch) {
      return states;
    }
    const next = new Map(states);
    next.set(pid, {
      injected: false,
      arch,
      phase: "failed",
      error: status.detail,
    });
    return next;
  }

  const enabled = status.state === "enabled";
  const phase = enabled ? "enabled" : "disabled";
  if (
    current?.injected &&
    current.arch === arch &&
    current.phase === phase &&
    !current.error
  ) return states;
  const next = new Map(states);
  next.set(pid, { injected: true, arch, phase });
  return next;
}

export default function ProcessManager() {
  const [processes, setProcesses] = useState<ProcessInfo[]>([]);
  const [search, setSearch] = useState("");
  const [icons, setIcons] = useState<Record<number, string>>({});
  const [speedMap, setSpeedMap] = useState<Map<number, SpeedState>>(new Map());
  const [selectedPid, setSelectedPid] = useState<number | null>(null);
  const { settings } = useSettings();
  const { speed, setSpeed, commitSpeed } = useSpeed();
  const { notify } = useSnackbar();
  const { t } = useTranslation();
  const speedMapRef = useRef(speedMap);
  const bridgeCommandPidsRef = useRef(new Set<number>());
  const bridgeOperationVersionRef = useRef(new Map<number, number>());
  const statusPollInFlightRef = useRef(false);
  const statusErrorRef = useRef(new Map<number, { detail: string; shownAt: number }>());

  function updateSpeedMap(updater: (current: Map<number, SpeedState>) => Map<number, SpeedState>) {
    const next = updater(speedMapRef.current);
    if (next === speedMapRef.current) return;
    speedMapRef.current = next;
    setSpeedMap(next);
  }

  function beginBridgeOperation(pid: number) {
    const version = (bridgeOperationVersionRef.current.get(pid) ?? 0) + 1;
    bridgeOperationVersionRef.current.set(pid, version);
    return version;
  }

  function reportStatusError(pid: number, error: unknown) {
    const detail = bridgeErrorMessage(error);
    console.error(`[status] bridge_get_status failed for pid ${pid}:`, detail);

    const now = Date.now();
    const previous = statusErrorRef.current.get(pid);
    if (previous?.detail === detail && now - previous.shownAt < 30_000) return;
    statusErrorRef.current.set(pid, { detail, shownAt: now });
    notify(`${t("process.statusFail")}: ${detail}`, "error", 10000);
  }

  function applyBridgeStatus(pid: number, arch: string, status: BridgeStatus) {
    if (status.state === "failed") {
      const previous = statusErrorRef.current.get(pid);
      if (previous?.detail !== status.detail) {
        statusErrorRef.current.set(pid, { detail: status.detail, shownAt: Date.now() });
        notify(`${t("process.injectFail")}: ${status.detail}`, "error", 10000);
      }
    } else {
      statusErrorRef.current.delete(pid);
    }
    updateSpeedMap(current => reconcileBridgeStatus(current, pid, arch, status));
  }

  const gears = useMemo(() => settings
    ? [1, 2, 3, 4, 5].map(i => (settings[`gear${i}Speed` as keyof typeof settings] as number) || 1)
    : [1, 2, 5, 10, 100],
  [settings]);

  // Toggle — optimistic update with rollback on failure
  async function toggle(pid: number, arch: string) {
    if (bridgeCommandPidsRef.current.has(pid)) return;
    bridgeCommandPidsRef.current.add(pid);
    beginBridgeOperation(pid);

    const cur = speedMapRef.current.get(pid);
    const wasOn = cur?.phase === "enabled";
    const wasInjected = cur?.phase !== "failed" && (cur?.injected ?? false);

    try {
      if (!wasOn) {
        // Turning ON
        if (!wasInjected) {
          updateSpeedMap(prev => {
            const next = new Map(prev);
            next.set(pid, { injected: false, arch, phase: "initializing" });
            return next;
          });
          const error = await invokeBridgeCommand("bridge_inject", { pid, arch });
          console.log("[toggle] bridge_inject result:", error ?? "OK");
          if (error) {
            if (bridgeOutcomeNeedsStatus(error)) {
              notify(error, "warning", 10000);
            } else {
              updateSpeedMap(prev => {
                const next = new Map(prev);
                next.set(pid, {
                  injected: false,
                  arch,
                  phase: "failed",
                  error,
                });
                return next;
              });
              notify(`${t("process.injectFail")}: ${error}`, "error", 10000);
            }
          } else {
            updateSpeedMap(prev => reconcileBridgeStatus(prev, pid, arch, { state: "enabled" }));
          }
        } else {
          updateSpeedMap(prev => {
            const next = new Map(prev);
            next.set(pid, { ...cur!, phase: "enabled", error: undefined });
            return next;
          });
          const error = await invokeBridgeCommand("bridge_enable", { pid, arch });
          if (error) {
            if (bridgeOutcomeNeedsStatus(error)) {
              notify(error, "warning", 10000);
            } else {
              updateSpeedMap(prev => {
                const next = new Map(prev);
                next.set(pid, cur!);
                return next;
              });
              notify(`${t("process.enableFail")}: ${error}`, "error", 10000);
            }
          } else {
            updateSpeedMap(prev => reconcileBridgeStatus(prev, pid, arch, { state: "enabled" }));
          }
        }
      } else {
        // Turning OFF
        updateSpeedMap(prev => {
          const next = new Map(prev);
          next.set(pid, { ...cur!, phase: "disabled", error: undefined });
          return next;
        });
        const error = await invokeBridgeCommand("bridge_disable", { pid, arch });
        if (error) {
          if (bridgeOutcomeNeedsStatus(error)) {
            notify(error, "warning", 10000);
          } else {
            updateSpeedMap(prev => {
              const next = new Map(prev);
              next.set(pid, cur!);
              return next;
            });
            notify(`${t("process.disableFail")}: ${error}`, "error", 10000);
          }
        } else {
          updateSpeedMap(prev => reconcileBridgeStatus(prev, pid, arch, { state: "disabled" }));
        }
      }
    } finally {
      bridgeCommandPidsRef.current.delete(pid);
    }
  }

  // Data fetch
  useEffect(() => { invoke<ProcessInfo[]>("get_process_list").then(setProcesses).catch(() => {}); }, []);
  useInterval(async () => { try { setProcesses(await invoke<ProcessInfo[]>("get_process_list_fast")); } catch {} }, 3000);
  useEffect(() => { if (search.trim()) { invoke<ProcessInfo[]>("get_process_list").then(setProcesses).catch(() => {}); } }, [search]);

  // Filter
  const filtered = useMemo(() => {
    const q = search.trim().toLowerCase();
    if (!q) return processes;
    return processes.filter(p => p.name.toLowerCase().includes(q) || p.pid.toString().includes(q) || (p.window_title && p.window_title.toLowerCase().includes(q)));
  }, [processes, search]);

  // Icons
  useEffect(() => {
    const pids = processes.map(p => p.pid).filter(pid => !(pid in icons));
    if (!pids.length) return;
    const CONCURRENCY = 6; let i = 0;
    async function worker() { while (i < pids.length) { const pid = pids[i++]; const v = await invoke<string | null>("get_process_icon", { pid }).then(u => u ?? "").catch(() => ""); setIcons(p => ({ ...p, [pid]: v })); } }
    for (let w = 0; w < CONCURRENCY; w++) worker();
  }, [processes]);

  const selectedProcess = useMemo(() =>
    selectedPid ? processes.find(p => p.pid === selectedPid) ?? null : null,
  [processes, selectedPid]);
  const selectedSpeedState = selectedPid ? speedMap.get(selectedPid) : undefined;

  // Query real injection status from bridge periodically for all tracked processes
  useInterval(async () => {
    if (statusPollInFlightRef.current || bridgeCommandPidsRef.current.size > 0) return;
    const snapshot = [...speedMapRef.current.entries()]
      .filter(([, state]) => state.phase !== "failed")
      .map(([pid, state]) => ({
        pid,
        state,
        version: bridgeOperationVersionRef.current.get(pid) ?? 0,
      }));
    if (snapshot.length === 0) return;

    statusPollInFlightRef.current = true;
    try {
      for (const { pid, state, version } of snapshot) {
        // The bridge has one command executor. Do not queue STATUS behind a
        // potentially long injection, and ignore a result that raced a toggle.
        if (bridgeCommandPidsRef.current.size > 0) break;
        try {
          const status = await invoke<BridgeStatus>("bridge_get_status", { pid, arch: state.arch });
          if (
            bridgeCommandPidsRef.current.has(pid) ||
            (bridgeOperationVersionRef.current.get(pid) ?? 0) !== version
          ) continue;
          const latest = speedMapRef.current.get(pid);
          if (!latest || latest.arch !== state.arch) continue;
          applyBridgeStatus(pid, state.arch, status);
        } catch (error) {
          if (
            !bridgeCommandPidsRef.current.has(pid) &&
            (bridgeOperationVersionRef.current.get(pid) ?? 0) === version
          ) {
            reportStatusError(pid, error);
          }
        }
      }
    } finally {
      statusPollInFlightRef.current = false;
    }
  }, 2000);

  // Instantly query status when selecting a new process
  useEffect(() => {
    const p = selectedProcess;
    if (
      !p ||
      bridgeCommandPidsRef.current.has(p.pid) ||
      speedMapRef.current.get(p.pid)?.phase === "failed"
    ) return;
    let cancelled = false;
    const version = bridgeOperationVersionRef.current.get(p.pid) ?? 0;
    invoke<BridgeStatus>("bridge_get_status", { pid: p.pid, arch: p.arch })
      .then(status => {
        if (
          cancelled ||
          bridgeCommandPidsRef.current.has(p.pid) ||
          (bridgeOperationVersionRef.current.get(p.pid) ?? 0) !== version
        ) return;
        applyBridgeStatus(p.pid, p.arch, status);
      })
      .catch(error => {
        if (
          !cancelled &&
          !bridgeCommandPidsRef.current.has(p.pid) &&
          (bridgeOperationVersionRef.current.get(p.pid) ?? 0) === version
        ) reportStatusError(p.pid, error);
      });
    return () => { cancelled = true; };
  }, [selectedPid]);

  return (
    <Box sx={{ height: "calc(100vh - 48px)", display: "flex", flexDirection: "column", overflow: "hidden" }}>
      <SpeedPanel speed={speed} gears={gears} onChange={setSpeed} onCommit={commitSpeed} />
      <Box sx={{ flex: 1, m: 1.5, overflow: "hidden" }}>
        <Splitter style={{ height: "100%" }}>
          <Splitter.Panel defaultSize="60%" min="300px">
            <ProcessTable
              processes={processes} filtered={filtered} search={search} onSearch={setSearch}
              icons={icons} speedStates={speedMap} selectedPid={selectedPid}
              onToggle={toggle} onSelect={setSelectedPid}
            />
          </Splitter.Panel>
          <Splitter.Panel min="250px">
            <ProcessDetail process={selectedProcess} speedState={selectedSpeedState} icons={icons} />
          </Splitter.Panel>
        </Splitter>
      </Box>
    </Box>
  );
}
