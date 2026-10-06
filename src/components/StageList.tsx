import { useTranslation } from "react-i18next";
import { Check, Loader2 } from "lucide-react";
import type { Busy } from "../store";
import { ProgressBar, cx } from "./ui";

export function StageList({ busy }: { busy: Busy }) {
  const { t } = useTranslation();
  const activeIndex = busy.stages.findIndex((s) => !s.done && s.progress > 0);
  const current = activeIndex === -1 ? busy.stages.findIndex((s) => !s.done) : activeIndex;

  return (
    <div className="flex flex-col gap-2">
      <div className="text-xs font-medium text-muted">{t(`busy.${busy.task}`)}…</div>
      {busy.stages.length === 0 && <ProgressBar value={0.05} />}
      {busy.stages.map((s, i) => {
        const running = i === current && !s.done;
        return (
          <div key={s.id} className="flex flex-col gap-1">
            <div className={cx("flex items-center gap-2 text-[13px]", !s.done && !running && "text-faint")}>
              {s.done ? (
                <Check size={14} className="text-ok" />
              ) : running ? (
                <Loader2 size={14} className="animate-spin text-accent" />
              ) : (
                <span className="inline-block h-3.5 w-3.5 rounded-full border border-line-strong" />
              )}
              <span className="flex-1">{t(`stages.${s.id}`, { defaultValue: s.id })}</span>
              {running && <span className="font-mono text-xs text-muted">{Math.round(s.progress * 100)}%</span>}
            </div>
            {running && <ProgressBar value={s.progress} />}
          </div>
        );
      })}
    </div>
  );
}
