import { useState, useEffect, useMemo } from "react";
import { useTranslation } from "react-i18next";
import { getVersion } from "@tauri-apps/api/app";
import { ThemeProvider, createTheme, CssBaseline, Box, Tabs, Tab, Typography, Paper, Switch, Chip } from "@mui/material";
import CheckCircleIcon from "@mui/icons-material/CheckCircle";
import ErrorIcon from "@mui/icons-material/Error";
import SpeedIcon from "@mui/icons-material/Speed";
import SettingsIcon from "@mui/icons-material/Settings";
import InfoIcon from "@mui/icons-material/Info";
import DarkModeIcon from "@mui/icons-material/DarkMode";
import LightModeIcon from "@mui/icons-material/LightMode";
import TitleBar from "./components/TitleBar";
import appIcon from "../src-tauri/icons/icon.png";
import ProcessManager from "./components/ProcessManager";
import SettingsManager from "./components/SettingsManager";
import { useShortcut } from "./hooks/useShortcut";
import { useTray } from "./hooks/useTray";
import { SnackbarProvider } from "./contexts/SnackbarContext";

import { useInterval } from "ahooks";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-shell";

function App() {
  const { t } = useTranslation();
  const [tab, setTab] = useState(0);
  const [cpuPct, setCpuPct] = useState(0);
  const [memPct, setMemPct] = useState(0);
  const [osVer, setOsVer] = useState("");
  const [version, setVersion] = useState("");
  const [b64, setB64] = useState<boolean | null>(null);
  const [b32, setB32] = useState<boolean | null>(null);
  const [gpuName, setGpuName] = useState("");
  const [gpuUsedMb, setGpuUsedMb] = useState(0);
  const [gpuTotalMb, setGpuTotalMb] = useState(0);
  const [darkMode, setDarkMode] = useState(true);

  // Sync Blueprint dark mode class
  useEffect(() => {
    document.body.classList.toggle("bp5-dark", darkMode);
  }, [darkMode]);

  const theme = useMemo(() => createTheme({
    palette: {
      mode: "dark",
      primary: { main: "#c9a96e" },        // 暗金 — 斗战神主色
      secondary: { main: "#8b1a1a" },      // 血红
      background: { default: "#0a0a0a", paper: "#181412" },
      text: { primary: "#e6dcc8", secondary: "#a8987a" },
      divider: "rgba(201, 169, 110, 0.18)",
    },
    typography: {
      fontFamily: '"Microsoft YaHei", "微软雅黑", "Inter", "Segoe UI", system-ui, -apple-system, sans-serif',
    },
    components: {
      MuiCssBaseline: {
        styleOverrides: {
          body: { overflow: "hidden", backgroundColor: "#0a0a0a" },
          "::-webkit-scrollbar": { width: 6 },
          "::-webkit-scrollbar-track": { background: "transparent" },
          "::-webkit-scrollbar-thumb": { background: "rgba(201, 169, 110, 0.18)", borderRadius: 3 },
          "::-webkit-scrollbar-thumb:hover": { background: "rgba(201, 169, 110, 0.32)" },
        },
      },
    },
  }), [darkMode]);

  const { init } = useShortcut();
  useEffect(() => { init(); }, [init]);

  useTray();

  useInterval(async () => {
    try {
      const s = await invoke<{ memory_pct: number; cpu_pct: number; os_version: string; gpu: { name: string; used_mb: number; total_mb: number } | null }>("get_system_stats");
      setMemPct(s.memory_pct);
      setCpuPct(s.cpu_pct);
      setOsVer(s.os_version);
      if (s.gpu) { setGpuName(s.gpu.name); setGpuUsedMb(s.gpu.used_mb); setGpuTotalMb(s.gpu.total_mb); }
    } catch {}
  }, 5000);

  useInterval(async () => {
    try {
      const [ok64, ok32] = await Promise.all([
        invoke<boolean>("bridge64_health").catch(() => false),
        invoke<boolean>("bridge32_health").catch(() => false),
      ]);
      setB64(ok64); setB32(ok32);
    } catch { setB64(false); setB32(false); }
  }, 5000);

  // Initial fetch
  useEffect(() => {
    invoke<{ memory_pct: number; cpu_pct: number; os_version: string; gpu: { name: string; used_mb: number; total_mb: number } | null }>("get_system_stats")
      .then(s => { setMemPct(s.memory_pct); setCpuPct(s.cpu_pct); setOsVer(s.os_version); if (s.gpu) { setGpuName(s.gpu.name); setGpuUsedMb(s.gpu.used_mb); setGpuTotalMb(s.gpu.total_mb); } }).catch(() => {});
    getVersion().then(setVersion).catch(() => {});
    invoke<boolean>("bridge64_health").catch(() => false).then(setB64);
    invoke<boolean>("bridge32_health").catch(() => false).then(setB32);
  }, []);

  return (
    <ThemeProvider theme={theme}>
      <CssBaseline />
      <SnackbarProvider>
      <Box sx={{ height: "100vh", display: "flex", flexDirection: "column", bgcolor: "background.default" }}>
        <TitleBar osVer={osVer} cpuPct={cpuPct} memPct={memPct} gpuName={gpuName} gpuUsedMb={gpuUsedMb} gpuTotalMb={gpuTotalMb} />

        <Box sx={{ flex: 1, display: "flex" }}>
          <Box sx={{ height: "calc(100vh - 48px)", borderRight: 1, borderColor: "divider", bgcolor: "background.paper", display: "flex", flexDirection: "column" }}>
            <Tabs orientation="vertical" value={tab} onChange={(_, v) => setTab(v)}
              sx={{ minWidth: 72, "& .MuiTab-root": { minHeight: 56 } }}>
              <Tab icon={<SpeedIcon />} label={t("app.tabs.speed")} />
              <Tab icon={<SettingsIcon />} label={t("app.tabs.settings")} />
              <Tab icon={<InfoIcon />} label={t("app.tabs.about")} />
            </Tabs>
            <Box sx={{ flex: 1 }} />

            <Box sx={{ display: "flex", flexDirection: "column", gap: 0.5, px: 1, pb: 0.5 }}>
              <BridgeChip ok={b64} label="B64" />
              <BridgeChip ok={b32} label="B32" />
            </Box>

            <Box sx={{ display: "flex", alignItems: "center", justifyContent: "center", pb: 1 }}>
              {darkMode ? <DarkModeIcon sx={{ fontSize: 14, color: "text.secondary" }} /> : <LightModeIcon sx={{ fontSize: 14, color: "text.secondary" }} />}
              <Switch size="small" checked={darkMode} onChange={(_, v) => setDarkMode(v)} />
            </Box>
          </Box>

          <Box sx={{ width: "calc(100vw - 72px)", display: "flex", flexDirection: "column", overflow: "hidden "}}>
            {tab === 0 && <ProcessManager />}

            {tab === 1 && <SettingsManager />}

            {tab === 2 && (
              <Box sx={{ flex: 1, width: "100%", overflow: "auto" }}>
                <Box sx={{ maxWidth: 400, mx: "auto", mt: 8, textAlign: "center", px: 2 }}>
                  <Box component="img" src={appIcon} sx={{ width: 80, height: 80, mb: 1 }} />
                  <Typography variant="h5" sx={{ fontWeight: 700, mb: 0.5, color: "primary.main" }}>{t("app.title")}</Typography>
                  <Typography variant="body2" color="text.secondary" sx={{ mb: 4 }}>
                    {t("about.description")}
                  </Typography>

                  <Paper elevation={0} sx={{ p: 2.5, bgcolor: "background.paper", border: 1, borderColor: "divider", textAlign: "left" }}>
                    <Box sx={{ display: "flex", justifyContent: "space-between", py: 1, borderBottom: 1, borderColor: "divider" }}>
                      <Typography variant="body2" sx={{ fontWeight: 600, color: "text.secondary" }}>{t("about.author")}</Typography>
                      <Typography variant="body2">wangneal</Typography>
                    </Box>
                    <Box sx={{ display: "flex", justifyContent: "space-between", py: 1, borderBottom: 1, borderColor: "divider" }}>
                      <Typography variant="body2" sx={{ fontWeight: 600, color: "text.secondary" }}>{t("about.license")}</Typography>
                      <Typography variant="body2">GPL v3</Typography>
                    </Box>
                    <Box sx={{ display: "flex", justifyContent: "space-between", py: 1, borderTop: 1, borderColor: "divider" }}>
                      <Typography variant="body2" sx={{ fontWeight: 600, color: "text.secondary" }}>{t("about.version")}</Typography>
                      <Typography variant="caption" color="text.secondary" sx={{ fontVariantNumeric: "tabular-nums" }}>v{version}</Typography>
                    </Box>
                    <Box sx={{ display: "flex", justifyContent: "space-between", py: 1, borderTop: 1, borderColor: "divider" }}>
                      <Typography variant="body2" sx={{ fontWeight: 600, color: "text.secondary" }}>{t("about.system")}</Typography>
                      <Typography variant="caption" color="text.secondary">{osVer}</Typography>
                    </Box>
                    <Box sx={{ display: "flex", justifyContent: "space-between", py: 1 }}>
                      <Typography variant="body2" sx={{ fontWeight: 600, color: "text.secondary" }}>GitHub</Typography>
                      <Typography variant="body2"
                        onClick={() => open("https://github.com/wangneal/DzsSpeedy")}
                        sx={{ color: "primary.main", cursor: "pointer", "&:hover": { textDecoration: "underline" } }}>
                        github.com/wangneal/DzsSpeedy
                      </Typography>
                    </Box>

                  </Paper>
                </Box>
              </Box>
            )}
          </Box>
        </Box>
      </Box>
      </SnackbarProvider>
    </ThemeProvider>
  );
}

function BridgeChip({ ok, label }: { ok: boolean | null; label: string }) {
  return (
    <Chip
      icon={ok === null ? undefined : ok ? <CheckCircleIcon /> : <ErrorIcon />}
      label={ok === null ? label : ok ? label : label}
      size="small"
      color={ok === null ? "default" : ok ? "success" : "error"}
      variant="filled"
      sx={{ height: 22, fontSize: "0.65rem" }}
    />
  );
}

export default App;
