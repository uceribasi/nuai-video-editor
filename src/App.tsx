import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { TriangleAlert, X } from "lucide-react";
import { useStore } from "./store";
import { player } from "./lib/player";
import { Header } from "./components/Header";
import { EmptyState } from "./components/EmptyState";
import { AnalyzePanel } from "./components/AnalyzePanel";
import { Player } from "./components/Player";
import { Transcript } from "./components/Transcript";
import { Timeline } from "./components/Timeline";
import { RightPanel } from "./components/RightPanel";
import { SettingsDialog } from "./components/SettingsDialog";
import { ExportDialog } from "./components/ExportDialog";
import { StageList } from "./components/StageList";
import { Button, IconButton } from "./components/ui";

const MEDIA_EXTENSIONS = [
  "mp4", "mov", "m4v", "mkv", "webm", "avi", "mts", "m2ts", "ts", "flv", "wmv", "mpg", "mpeg",
  "mp3", "wav", "m4a", "aac", "flac", "ogg", "opus",
];

function isTyping(target: EventTarget | null) {
  const el = target as HTMLElement | null;
  return !!el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.tagName === "SELECT" || el.isContentEditable);
}

export default function App() {
  const { t } = useTranslation();
  const ready = useStore((s) => !!s.settings);
  const media = useStore((s) => s.media);
  const busy = useStore((s) => s.busy);
  const error = useStore((s) => s.error);
  const settingsOpen = useStore((s) => s.settingsOpen);
  const exportOpen = useStore((s) => s.exportState.open);
  const init = useStore((s) => s.init);
  const openFile = useStore((s) => s.openFile);
  const cancel = useStore((s) => s.cancel);
  const dismissError = useStore((s) => s.dismissError);
  const [dragging, setDragging] = useState(false);

  useEffect(() => {
    void init().catch((e) => useStore.setState({ error: String(e) }));
  }, [init]);

  const pickFile = useCallback(async () => {
    const path = await openDialog({
      multiple: false,
      directory: false,
      filters: [{ name: "Video / Audio", extensions: MEDIA_EXTENSIONS }],
    });
    if (typeof path === "string") void openFile(path);
  }, [openFile]);

  useEffect(() => {
    const unlisten = getCurrentWebview().onDragDropEvent((e) => {
      const p = e.payload;
      if (p.type === "enter" || p.type === "over") setDragging(true);
      else if (p.type === "leave") setDragging(false);
      else if (p.type === "drop") {
        setDragging(false);
        const file = p.paths.find((f) => MEDIA_EXTENSIONS.includes(f.split(".").pop()?.toLowerCase() ?? ""));
        if (file && !useStore.getState().busy) void openFile(file);
      }
    });
    return () => {
      void unlisten.then((f) => f());
    };
  }, [openFile]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const s = useStore.getState();
      const mod = e.metaKey || e.ctrlKey;
      if (mod && e.key.toLowerCase() === "o") {
        e.preventDefault();
        if (!s.busy) void pickFile();
        return;
      }
      if (mod && e.key === ",") {
        e.preventDefault();
        s.openSettings();
        return;
      }
      if (isTyping(e.target) || s.settingsOpen || s.exportState.open) return;
      if (mod && e.key.toLowerCase() === "z") {
        e.preventDefault();
        if (e.shiftKey) s.redo();
        else s.undo();
        return;
      }
      if (mod && e.key.toLowerCase() === "e") {
        e.preventDefault();
        if (s.project && !s.busy) void s.openExport();
        return;
      }
      if (mod) return;
      switch (e.key) {
        case " ":
          e.preventDefault();
          player.toggle();
          break;
        case "ArrowLeft":
          e.preventDefault();
          player.nudge(e.shiftKey ? -5 : -1);
          break;
        case "ArrowRight":
          e.preventDefault();
          player.nudge(e.shiftKey ? 5 : 1);
          break;
        case "p":
        case "P":
          s.setPreviewEdits(!s.previewEdits);
          break;
        case "x":
        case "X":
          if (s.selectedCutId) s.toggleCut(s.selectedCutId);
          break;
        case "Delete":
        case "Backspace": {
          const cut = s.project?.cuts.find((c) => c.id === s.selectedCutId);
          if (cut?.source === "user") s.removeCut(cut.id);
          else if (cut) s.toggleCut(cut.id, false);
          break;
        }
        case "Escape":
          s.selectCut(null);
          break;
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [pickFile]);

  if (!ready) return <div className="h-full bg-bg" />;

  return (
    <div className="flex h-full flex-col">
      <Header onOpen={() => void pickFile()} />

      {media ? (
        <>
          <div className="flex min-h-0 flex-1">
            <AnalyzePanel />
            <main className="flex min-w-0 flex-1 flex-col">
              <div className="flex min-h-0 flex-[3] flex-col">
                <Player />
              </div>
              <div className="flex min-h-0 flex-[2] flex-col border-t border-line bg-bg">
                <div className="px-6 pt-3 text-xs font-medium uppercase tracking-wide text-faint">{t("transcript.title")}</div>
                <Transcript />
              </div>
            </main>
            <RightPanel />
          </div>
          <Timeline />
        </>
      ) : (
        <EmptyState onOpen={() => void pickFile()} dragging={dragging} />
      )}

      {busy?.task === "open" && (
        <div className="fixed inset-0 z-40 flex items-center justify-center bg-black/50">
          <div className="flex w-[340px] flex-col gap-4 rounded-xl border border-line bg-panel p-5 shadow-2xl">
            <StageList busy={busy} />
            <Button onClick={cancel}>{t("common.cancel")}</Button>
          </div>
        </div>
      )}

      {dragging && media && (
        <div className="pointer-events-none fixed inset-3 z-30 rounded-2xl border-2 border-dashed border-accent bg-accent-soft" />
      )}

      {error && (
        <div className="fixed bottom-4 right-4 z-50 flex max-w-[460px] gap-3 rounded-lg border border-danger/40 bg-panel p-3 shadow-2xl">
          <TriangleAlert size={18} className="mt-0.5 shrink-0 text-danger" />
          <div className="min-w-0 flex-1">
            <div className="font-medium">{t("error.title")}</div>
            <div className="selectable mt-0.5 whitespace-pre-wrap break-words text-xs text-muted">{error}</div>
          </div>
          <IconButton label={t("common.close")} onClick={dismissError} className="h-6 w-6">
            <X size={14} />
          </IconButton>
        </div>
      )}

      {settingsOpen && <SettingsDialog />}
      {exportOpen && <ExportDialog />}
    </div>
  );
}
