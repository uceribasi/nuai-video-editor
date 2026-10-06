import { useEffect, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { Check, CheckCircle2, Cpu, Download, KeyRound, Loader2, RefreshCw, Sparkles, SquareTerminal, Trash2, TriangleAlert, X } from "lucide-react";
import { useStore, type SettingsTab } from "../store";
import { api, errorText } from "../lib/api";
import type { AiProvider, KeyProvider, ModelInfo, Settings } from "../lib/types";
import { Button, Modal, ProgressBar, Select, TextInput, Toggle, cx } from "./ui";
import { VoiceSettings } from "./VoiceSettings";

const CUSTOM = "__custom__";

const LANGUAGES = ["auto", "tr", "en", "de", "fr", "es", "it", "pt", "nl", "pl", "ru", "uk", "ar", "fa", "hi", "ja", "ko", "zh"];

const COMPATIBLE_PRESETS = [
  { name: "Ollama", url: "http://localhost:11434/v1" },
  { name: "LM Studio", url: "http://localhost:1234/v1" },
  { name: "OpenRouter", url: "https://openrouter.ai/api/v1" },
  { name: "Gemini", url: "https://generativelanguage.googleapis.com/v1beta/openai" },
];

function Field({ label, hint, children }: { label: string; hint?: ReactNode; children: ReactNode }) {
  return (
    <div className="flex flex-col gap-1.5">
      <span className="text-xs font-medium text-muted">{label}</span>
      {children}
      {hint && <span className="text-xs text-faint">{hint}</span>}
    </div>
  );
}

function ChoiceCard({
  selected,
  onClick,
  title,
  description,
  icon,
}: {
  selected: boolean;
  onClick: () => void;
  title: string;
  description: string;
  icon?: ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={cx(
        "flex items-start gap-2.5 rounded-lg border p-3 text-left transition-colors",
        selected ? "border-accent bg-accent-soft" : "border-line hover:border-line-strong hover:bg-hover",
      )}
    >
      {icon && <span className={cx("mt-0.5", selected ? "text-accent" : "text-faint")}>{icon}</span>}
      <span className="flex flex-col">
        <span className="font-medium">{title}</span>
        <span className="text-xs text-muted">{description}</span>
      </span>
    </button>
  );
}

function GeneralTab({ s, update }: { s: Settings; update: (p: (s: Settings) => Settings) => void }) {
  const { t } = useTranslation();
  const status = useStore((st) => st.status);
  return (
    <div className="flex flex-col gap-5">
      <Field label={t("settings.language")}>
        <Select
          value={s.uiLanguage}
          onChange={(v) => update((x) => ({ ...x, uiLanguage: v as Settings["uiLanguage"] }))}
          options={[
            { value: "system", label: t("settings.system") },
            { value: "en", label: "English" },
            { value: "tr", label: "Türkçe" },
          ]}
        />
      </Field>
      <Field label={t("settings.defaultMode")}>
        <div className="grid grid-cols-2 gap-2">
          <ChoiceCard
            selected={s.mode === "review"}
            onClick={() => update((x) => ({ ...x, mode: "review" }))}
            title={t("header.review")}
            description={t("header.reviewHint")}
          />
          <ChoiceCard
            selected={s.mode === "auto"}
            onClick={() => update((x) => ({ ...x, mode: "auto" }))}
            title={t("header.auto")}
            description={t("header.autoHint")}
          />
        </div>
      </Field>
      <Field
        label={t("settings.ffmpeg")}
        hint={
          status?.ffmpeg ? (
            <span className="text-ok">{t("settings.ffmpegFound", { path: status.ffmpeg.ffmpeg })}</span>
          ) : (
            <span className="text-warn">{status?.ffmpegError}</span>
          )
        }
      >
        <div className="flex gap-2">
          <TextInput
            value={s.ffmpegPath}
            placeholder={t("settings.ffmpegPlaceholder")}
            onChange={(e) => update((x) => ({ ...x, ffmpegPath: e.target.value }))}
          />
          <Button
            onClick={async () => {
              const p = await openDialog({ multiple: false, directory: false });
              if (typeof p === "string") update((x) => ({ ...x, ffmpegPath: p }));
            }}
          >
            {t("common.browse")}
          </Button>
        </div>
      </Field>
    </div>
  );
}

function AiTab({ s, update }: { s: Settings; update: (p: (s: Settings) => Settings) => void }) {
  const { t } = useTranslation();
  const status = useStore((st) => st.status);
  const refreshStatus = useStore((st) => st.refreshStatus);
  const [keyDraft, setKeyDraft] = useState("");
  const [models, setModels] = useState<ModelInfo[]>([]);
  const [loadingModels, setLoadingModels] = useState(false);
  const [manual, setManual] = useState(false);
  const [test, setTest] = useState<{ ok: boolean; text: string } | "running" | null>(null);
  const [error, setError] = useState<string | null>(null);
  const ai = s.ai;
  const provider = ai.provider;
  const setAi = (patch: Partial<Settings["ai"]>) => update((x) => ({ ...x, ai: { ...x.ai, ...patch } }));
  const keyProvider: KeyProvider | null = provider === "none" || provider === "codexCli" ? null : provider;
  const hasKey = keyProvider !== null && status?.apiKeys[keyProvider];
  const modelKey =
    provider === "anthropic"
      ? "anthropicModel"
      : provider === "openai"
        ? "openaiModel"
        : provider === "codexCli"
          ? "codexModel"
          : "compatibleModel";

  const saveKey = async (key: string) => {
    if (!keyProvider) return;
    setError(null);
    try {
      await api.setApiKey(keyProvider, key);
      setKeyDraft("");
      await refreshStatus();
    } catch (e) {
      setError(errorText(e));
    }
  };

  const loadModels = async (quiet = false) => {
    setLoadingModels(true);
    if (!quiet) setError(null);
    try {
      setModels(await api.listAiModels(ai));
    } catch (e) {
      if (!quiet) setError(errorText(e));
    } finally {
      setLoadingModels(false);
    }
  };

  // Fetch the model list as soon as the provider can answer: Codex reads its local
  // cache, API providers need a saved key, local servers are tried quietly.
  useEffect(() => {
    setModels([]);
    setManual(false);
    if (provider === "codexCli" || provider === "compatible" || hasKey) void loadModels(true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [provider, hasKey, ai.compatibleBaseUrl]);

  const current = ai[modelKey];
  const known = models.find((m) => m.id === current);
  const selectedModel = known ?? (provider === "codexCli" && !current ? models[0] : undefined);
  const efforts = selectedModel?.efforts?.length
    ? selectedModel.efforts
    : provider === "anthropic"
      ? ["low", "medium", "high", "xhigh", "max"]
      : ["low", "medium", "high"];

  const chooseModel = (id: string) => {
    const m = models.find((x) => x.id === id) ?? (provider === "codexCli" && !id ? models[0] : undefined);
    const patch: Partial<Settings["ai"]> = { [modelKey]: id };
    if (m?.efforts?.length && !m.efforts.includes(ai.effort)) patch.effort = m.defaultEffort ?? m.efforts[0];
    setAi(patch);
  };

  const runTest = async () => {
    setTest("running");
    try {
      const model = await api.testAi(ai);
      setTest({ ok: true, text: t("settings.ai.testOk", { model }) });
    } catch (e) {
      setTest({ ok: false, text: errorText(e) });
    }
  };

  const providers: { id: AiProvider; icon: ReactNode }[] = [
    { id: "none", icon: <Cpu size={16} /> },
    { id: "codexCli", icon: <SquareTerminal size={16} /> },
    { id: "anthropic", icon: <Sparkles size={16} /> },
    { id: "openai", icon: <Sparkles size={16} /> },
    { id: "compatible", icon: <Sparkles size={16} /> },
  ];

  return (
    <div className="flex flex-col gap-5">
      <Field label={t("settings.ai.provider")}>
        <div className="grid grid-cols-2 gap-2">
          {providers.map((p) => (
            <ChoiceCard
              key={p.id}
              selected={provider === p.id}
              onClick={() => {
                setAi({ provider: p.id });
                setModels([]);
                setTest(null);
                setError(null);
              }}
              icon={p.icon}
              title={t(`settings.ai.${p.id}`)}
              description={t(`settings.ai.${p.id}Desc`)}
            />
          ))}
        </div>
      </Field>

      {provider === "compatible" && (
        <Field label={t("settings.ai.baseUrl")}>
          <TextInput value={ai.compatibleBaseUrl} onChange={(e) => setAi({ compatibleBaseUrl: e.target.value })} />
          <div className="flex flex-wrap gap-1.5">
            {COMPATIBLE_PRESETS.map((p) => (
              <button
                key={p.name}
                type="button"
                onClick={() => setAi({ compatibleBaseUrl: p.url })}
                className={cx(
                  "rounded-full border px-2.5 py-0.5 text-xs",
                  ai.compatibleBaseUrl === p.url ? "border-accent text-accent" : "border-line text-muted hover:text-fg",
                )}
              >
                {p.name}
              </button>
            ))}
          </div>
        </Field>
      )}

      {provider === "codexCli" && (
        <Field
          label={t("settings.ai.codexPath")}
          hint={
            status?.codex.path ? (
              status.codex.loggedIn ? (
                <span className="flex items-center gap-1 text-ok">
                  <CheckCircle2 size={12} /> {t("settings.ai.codexFound", { path: status.codex.path, version: status.codex.version ?? "?" })}
                </span>
              ) : (
                <span className="text-warn">{t("settings.ai.codexNotLoggedIn")}</span>
              )
            ) : (
              <span className="text-warn">{t("settings.ai.codexMissing")}</span>
            )
          }
        >
          <div className="flex gap-2">
            <TextInput
              value={ai.codexPath}
              placeholder={t("settings.ai.codexPathPlaceholder")}
              onChange={(e) => setAi({ codexPath: e.target.value })}
              onBlur={() => void refreshStatus()}
            />
            <Button
              onClick={async () => {
                const p = await openDialog({ multiple: false, directory: false });
                if (typeof p === "string") {
                  setAi({ codexPath: p });
                  setTimeout(() => void refreshStatus(), 500);
                }
              }}
            >
              {t("common.browse")}
            </Button>
          </div>
        </Field>
      )}

      {keyProvider && (
          <Field
            label={provider === "compatible" ? t("settings.ai.apiKeyOptional") : t("settings.ai.apiKey")}
            hint={
              hasKey ? (
                <span className="flex items-center gap-1 text-ok">
                  <KeyRound size={12} /> {t("settings.ai.keySaved")}
                </span>
              ) : (
                t("settings.ai.keyMissing")
              )
            }
          >
            <div className="flex gap-2">
              <TextInput
                type="password"
                value={keyDraft}
                placeholder={hasKey ? "••••••••••••" : provider === "anthropic" ? "sk-ant-…" : "sk-…"}
                onChange={(e) => setKeyDraft(e.target.value)}
                autoComplete="off"
                spellCheck={false}
              />
              <Button variant="primary" disabled={!keyDraft.trim()} onClick={() => void saveKey(keyDraft)}>
                {t("settings.ai.saveKey")}
              </Button>
              {hasKey && (
                <Button variant="danger" onClick={() => void saveKey("")} title={t("settings.ai.removeKey")}>
                  <Trash2 size={14} />
                </Button>
              )}
            </div>
          </Field>
      )}

      {provider !== "none" && (
        <>
          <Field
            label={t("settings.ai.model")}
            hint={
              selectedModel?.description ??
              (current && models.length > 0 && !known ? (
                <span className="text-warn">{t("settings.ai.modelNotListed")}</span>
              ) : undefined)
            }
          >
            {models.length > 0 && !manual ? (
              <div className="flex gap-2">
                <Select
                  value={current}
                  onChange={(v) => (v === CUSTOM ? setManual(true) : chooseModel(v))}
                  options={[
                    ...(provider === "codexCli" ? [{ value: "", label: t("settings.ai.autoModel", { name: models[0].name }) }] : []),
                    ...(current && !known ? [{ value: current, label: `${current} — ${t("settings.ai.notListed")}` }] : []),
                    ...models.map((m) => ({ value: m.id, label: m.name === m.id ? m.id : `${m.name} · ${m.id}` })),
                    { value: CUSTOM, label: t("settings.ai.customModel") },
                  ]}
                />
                <Button onClick={() => void loadModels()} disabled={loadingModels} title={t("settings.ai.refreshModels")}>
                  <RefreshCw size={13} className={loadingModels ? "animate-spin" : undefined} />
                </Button>
              </div>
            ) : (
              <div className="flex gap-2">
                <TextInput
                  value={current}
                  placeholder={provider === "codexCli" ? t("settings.ai.codexModelPlaceholder") : t("settings.ai.modelPlaceholder")}
                  onChange={(e) => setAi({ [modelKey]: e.target.value })}
                  spellCheck={false}
                />
                {models.length > 0 ? (
                  <Button onClick={() => setManual(false)}>{t("settings.ai.backToList")}</Button>
                ) : (
                  <Button onClick={() => void loadModels()} disabled={loadingModels}>
                    {loadingModels && <Loader2 size={13} className="animate-spin" />}
                    {t("settings.ai.loadModels")}
                  </Button>
                )}
              </div>
            )}
          </Field>

          {provider !== "compatible" && (
            <Field label={t("settings.ai.effort")}>
              <Select
                value={efforts.includes(ai.effort) ? ai.effort : (selectedModel?.defaultEffort ?? efforts[0])}
                onChange={(v) => setAi({ effort: v })}
                options={efforts.map((e) => ({ value: e, label: t(`settings.ai.efforts.${e}`, { defaultValue: e }) }))}
              />
            </Field>
          )}

          <div className="flex items-center gap-3">
            <Button onClick={() => void runTest()} disabled={test === "running"}>
              {test === "running" && <Loader2 size={13} className="animate-spin" />}
              {t("settings.ai.test")}
            </Button>
            {test && test !== "running" && (
              <span className={cx("flex items-center gap-1.5 text-xs", test.ok ? "text-ok" : "text-danger")}>
                {test.ok ? <CheckCircle2 size={14} /> : <TriangleAlert size={14} />}
                <span className="selectable">{test.text}</span>
              </span>
            )}
          </div>
          {error && <p className="selectable text-xs text-danger">{error}</p>}
          <p className="text-xs text-faint">{t("settings.ai.privacy")}</p>
        </>
      )}
      <p className="rounded-md border border-line bg-raised/50 p-3 text-xs text-muted">{t("settings.ai.subscriptions")}</p>
    </div>
  );
}

function TranscriptionTab({ s, update }: { s: Settings; update: (p: (s: Settings) => Settings) => void }) {
  const { t } = useTranslation();
  const status = useStore((st) => st.status);
  const downloads = useStore((st) => st.downloads);
  const downloadModel = useStore((st) => st.downloadModel);
  const refreshStatus = useStore((st) => st.refreshStatus);
  const tr = s.transcription;
  const setTr = (patch: Partial<Settings["transcription"]>) =>
    update((x) => ({ ...x, transcription: { ...x.transcription, ...patch } }));

  return (
    <div className="flex flex-col gap-5">
      <Field label={t("settings.transcription.language")}>
        <Select
          value={tr.language}
          onChange={(v) => setTr({ language: v })}
          options={LANGUAGES.map((l) => ({ value: l, label: t(`languages.${l}`) }))}
        />
      </Field>
      <Field
        label={t("settings.transcription.models")}
        hint={status && t("settings.transcription.folder", { dir: status.modelsDir })}
      >
        <div className="flex flex-col divide-y divide-line rounded-lg border border-line">
          {status?.models.map((m) => {
            const active = !tr.customModelPath.trim() && tr.model === m.id;
            const progress = downloads[m.id];
            return (
              <div key={m.id} className="flex items-center gap-3 px-3 py-2.5">
                <div className="flex min-w-0 flex-1 flex-col">
                  <span className="flex items-center gap-2 font-mono text-[12.5px]">
                    {m.id}
                    {m.recommended && (
                      <span className="rounded bg-accent-soft px-1.5 font-sans text-[10px] font-medium text-accent">
                        {t("settings.transcription.recommended")}
                      </span>
                    )}
                  </span>
                  <span className="text-xs text-faint">{m.sizeMb} MB</span>
                  {progress !== undefined && <ProgressBar value={progress} className="mt-1.5" />}
                </div>
                {progress !== undefined ? (
                  <Button size="sm" onClick={() => void api.cancelDownload()}>
                    <X size={13} />
                    {t("common.cancel")}
                  </Button>
                ) : m.installed ? (
                  <>
                    {active ? (
                      <span className="flex items-center gap-1 text-xs text-ok">
                        <Check size={13} />
                        {t("settings.transcription.active")}
                      </span>
                    ) : (
                      <Button size="sm" onClick={() => setTr({ model: m.id, customModelPath: "" })}>
                        {t("settings.transcription.use")}
                      </Button>
                    )}
                    <Button
                      size="sm"
                      variant="ghost"
                      title={t("settings.transcription.delete")}
                      onClick={async () => {
                        await api.deleteModel(m.id);
                        await refreshStatus();
                      }}
                    >
                      <Trash2 size={13} />
                    </Button>
                  </>
                ) : (
                  <Button
                    size="sm"
                    onClick={() => {
                      setTr({ model: m.id, customModelPath: "" });
                      void downloadModel(m.id);
                    }}
                  >
                    <Download size={13} />
                    {t("settings.transcription.download")}
                  </Button>
                )}
              </div>
            );
          })}
        </div>
      </Field>
      <Field label={t("settings.transcription.custom")}>
        <div className="flex gap-2">
          <TextInput
            value={tr.customModelPath}
            placeholder={t("settings.transcription.customPlaceholder")}
            onChange={(e) => setTr({ customModelPath: e.target.value })}
          />
          <Button
            onClick={async () => {
              const p = await openDialog({ multiple: false, filters: [{ name: "ggml", extensions: ["bin"] }] });
              if (typeof p === "string") setTr({ customModelPath: p });
            }}
          >
            {t("common.browse")}
          </Button>
        </div>
      </Field>
      <label className="flex items-center justify-between">
        <span>{t("settings.transcription.gpu")}</span>
        <Toggle checked={tr.useGpu} onChange={(v) => setTr({ useGpu: v })} />
      </label>
    </div>
  );
}

function ExportTab({ s, update }: { s: Settings; update: (p: (s: Settings) => Settings) => void }) {
  const { t } = useTranslation();
  const ex = s.export;
  const setEx = (patch: Partial<Settings["export"]>) => update((x) => ({ ...x, export: { ...x.export, ...patch } }));
  return (
    <div className="flex flex-col gap-5">
      <Field label={t("settings.export.codec")}>
        <Select
          value={ex.codec}
          onChange={(v) => setEx({ codec: v as Settings["export"]["codec"] })}
          options={[
            { value: "h264", label: t("settings.export.h264") },
            { value: "hevc", label: t("settings.export.hevc") },
          ]}
        />
      </Field>
      <Field label={t("settings.export.quality")}>
        <Select
          value={ex.quality}
          onChange={(v) => setEx({ quality: v as Settings["export"]["quality"] })}
          options={[
            { value: "high", label: t("settings.export.high") },
            { value: "balanced", label: t("settings.export.balanced") },
            { value: "small", label: t("settings.export.small") },
          ]}
        />
      </Field>
      <Field label={t("settings.export.folder")}>
        <div className="flex gap-2">
          <TextInput readOnly value={ex.outputDir || t("settings.export.sameFolder")} className="text-muted" />
          <Button
            onClick={async () => {
              const p = await openDialog({ directory: true, multiple: false });
              if (typeof p === "string") setEx({ outputDir: p });
            }}
          >
            {t("settings.export.choose")}
          </Button>
          {ex.outputDir && <Button onClick={() => setEx({ outputDir: "" })}>{t("settings.export.reset")}</Button>}
        </div>
      </Field>
      <Field label={t("settings.export.suffix")}>
        <TextInput value={ex.suffix} onChange={(e) => setEx({ suffix: e.target.value })} className="font-mono" />
      </Field>
    </div>
  );
}

export function SettingsDialog() {
  const { t } = useTranslation();
  const settings = useStore((s) => s.settings);
  const tab = useStore((s) => s.settingsTab);
  const close = useStore((s) => s.closeSettings);
  const update = useStore((s) => s.updateSettings);
  const openSettings = useStore((s) => s.openSettings);
  if (!settings) return null;

  const tabs: SettingsTab[] = ["general", "ai", "transcription", "voice", "export"];
  return (
    <Modal title={t("settings.title")} onClose={close} width={760}>
      <div className="flex min-h-[460px]">
        <nav className="flex w-44 shrink-0 flex-col gap-0.5 border-r border-line p-2">
          {tabs.map((id) => (
            <button
              key={id}
              type="button"
              onClick={() => openSettings(id)}
              className={cx(
                "rounded-md px-3 py-1.5 text-left",
                tab === id ? "bg-raised text-fg" : "text-muted hover:bg-hover hover:text-fg",
              )}
            >
              {t(`settings.tabs.${id}`)}
            </button>
          ))}
        </nav>
        <div className="min-w-0 flex-1 p-5">
          {tab === "general" && <GeneralTab s={settings} update={update} />}
          {tab === "ai" && <AiTab s={settings} update={update} />}
          {tab === "transcription" && <TranscriptionTab s={settings} update={update} />}
          {tab === "export" && <ExportTab s={settings} update={update} />}
          {tab === "voice" && <VoiceSettings />}
        </div>
      </div>
    </Modal>
  );
}
