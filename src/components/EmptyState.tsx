import { useTranslation } from "react-i18next";
import { CheckCircle2, Circle, Download, FolderOpen, Sparkles, TriangleAlert, Film } from "lucide-react";
import { useStore } from "../store";
import { Button, ProgressBar } from "./ui";

export function EmptyState({ onOpen, dragging }: { onOpen: () => void; dragging: boolean }) {
  const { t } = useTranslation();
  const status = useStore((s) => s.status);
  const settings = useStore((s) => s.settings);
  const downloads = useStore((s) => s.downloads);
  const downloadModel = useStore((s) => s.downloadModel);
  const openSettings = useStore((s) => s.openSettings);

  const modelId = settings?.transcription.model;
  const customModel = settings?.transcription.customModelPath.trim();
  const model = status?.models.find((m) => m.id === modelId);
  const modelReady = !!customModel || !!model?.installed;
  const provider = settings?.ai.provider ?? "none";
  const downloading = modelId ? downloads[modelId] : undefined;

  return (
    <div className="flex flex-1 items-center justify-center p-10">
      <div className="flex w-full max-w-[560px] flex-col items-center text-center">
        <div
          className={`mb-6 flex w-full flex-col items-center rounded-2xl border-2 border-dashed px-10 py-12 transition-colors ${
            dragging ? "border-accent bg-accent-soft" : "border-line-strong"
          }`}
        >
          <div className="mb-4 flex h-14 w-14 items-center justify-center rounded-2xl bg-raised">
            <Film size={26} className="text-accent" />
          </div>
          <h1 className="mb-2 text-lg font-semibold">{t("empty.title")}</h1>
          <p className="mb-6 max-w-[420px] text-muted">{t("empty.subtitle")}</p>
          <Button variant="primary" size="lg" onClick={onOpen}>
            <FolderOpen size={16} />
            {t("empty.open")}
          </Button>
          <span className="mt-3 text-xs text-faint">{t("empty.dropHint")}</span>
        </div>

        {status && (
          <div className="w-full rounded-xl border border-line bg-panel p-4 text-left">
            <div className="mb-3 text-xs font-medium uppercase tracking-wide text-faint">{t("empty.setup")}</div>
            <ul className="flex flex-col gap-2.5">
              <li className="flex items-center gap-2.5">
                {status.ffmpeg ? (
                  <CheckCircle2 size={16} className="text-ok" />
                ) : (
                  <TriangleAlert size={16} className="text-warn" />
                )}
                <span className="flex-1">{status.ffmpeg ? t("empty.ffmpegOk") : t("empty.ffmpegMissing")}</span>
                {!status.ffmpeg && (
                  <Button size="sm" onClick={() => openSettings("general")}>
                    {t("empty.configure")}
                  </Button>
                )}
              </li>
              <li className="flex items-center gap-2.5">
                {modelReady ? (
                  <CheckCircle2 size={16} className="text-ok" />
                ) : (
                  <Circle size={16} className="text-warn" />
                )}
                <div className="flex flex-1 flex-col gap-1.5">
                  <span>
                    {modelReady
                      ? t("empty.modelOk", { name: customModel ? customModel.split(/[\\/]/).pop() : modelId })
                      : t("empty.modelMissing", { size: `${model?.sizeMb ?? "?"} MB` })}
                  </span>
                  {downloading !== undefined && <ProgressBar value={downloading} />}
                </div>
                {!modelReady && modelId && downloading === undefined && (
                  <Button size="sm" onClick={() => void downloadModel(modelId)}>
                    <Download size={13} />
                    {t("empty.download")}
                  </Button>
                )}
              </li>
              <li className="flex items-center gap-2.5">
                <Sparkles size={16} className={provider === "none" ? "text-faint" : "text-accent"} />
                <span className="flex-1">
                  {provider === "none"
                    ? t("empty.aiOff")
                    : t("empty.aiOn", { provider: t(`settings.ai.${provider}`) })}
                </span>
                <Button size="sm" onClick={() => openSettings("ai")}>
                  {t("empty.configure")}
                </Button>
              </li>
            </ul>
          </div>
        )}
      </div>
    </div>
  );
}
