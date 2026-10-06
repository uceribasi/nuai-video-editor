/**
 * Caption drawing shared by the live preview and burned-in export. Sizes are relative
 * to the video, so what the preview shows is what gets burned. The browser's text
 * engine does the shaping, which covers every script (Arabic, Devanagari, CJK…).
 */
import type { CaptionSize, CaptionStyle } from "./types";

export interface CaptionLook {
  size: CaptionSize;
  style: CaptionStyle;
}

const SIZE: Record<CaptionSize, number> = { small: 0.045, medium: 0.056, large: 0.07 };
const FONT = '-apple-system, BlinkMacSystemFont, "Segoe UI", "Noto Sans", Roboto, Arial, sans-serif';

function metrics(w: number, h: number, look: CaptionLook) {
  // Relative to the short side so vertical videos don't get giant text.
  const font = Math.max(10, Math.round(Math.min(w, h) * SIZE[look.size]));
  return { font, margin: Math.round(h * 0.06), maxWidth: w * 0.9 };
}

/**
 * Draws `text` centered, with its last line `margin` above `bottom`. `w`/`h` are the
 * video's size, which sets the type size; `bottom` lets a short strip be rendered.
 */
export function drawCaption(
  ctx: CanvasRenderingContext2D,
  text: string,
  w: number,
  h: number,
  look: CaptionLook,
  bottom = h,
) {
  const lines = text.split("\n").map((l) => l.trim()).filter(Boolean);
  if (!lines.length) return;
  const m = metrics(w, h, look);
  let font = m.font;
  ctx.font = `600 ${font}px ${FONT}`;
  const widest = Math.max(...lines.map((l) => ctx.measureText(l).width));
  if (widest > m.maxWidth) {
    font = Math.floor((font * m.maxWidth) / widest);
    ctx.font = `600 ${font}px ${FONT}`;
  }
  const lineHeight = Math.round(font * 1.28);
  const x = w / 2;
  let y = bottom - m.margin - (lines.length - 1) * lineHeight;
  ctx.textAlign = "center";
  ctx.textBaseline = "alphabetic";
  for (const line of lines) {
    if (look.style === "box") {
      const tw = ctx.measureText(line).width;
      const padX = font * 0.35;
      const top = y - font * 0.98;
      ctx.fillStyle = "rgba(0, 0, 0, 0.72)";
      ctx.beginPath();
      ctx.roundRect(x - tw / 2 - padX, top, tw + padX * 2, lineHeight, font * 0.18);
      ctx.fill();
    } else {
      ctx.lineJoin = "round";
      ctx.lineWidth = Math.max(2, font * 0.16);
      ctx.strokeStyle = "rgba(0, 0, 0, 0.92)";
      ctx.strokeText(line, x, y);
    }
    ctx.fillStyle = "#ffffff";
    ctx.fillText(line, x, y);
    y += lineHeight;
  }
}

/** Height of the transparent strip burned at the bottom of the frame. */
export function stripHeight(w: number, h: number, look: CaptionLook, lines: number): number {
  const m = metrics(w, h, look);
  return Math.min(h, Math.ceil(m.margin + m.font * 1.28 * (lines + 0.5)));
}

/** A transparent PNG strip (base64) with the caption drawn as it will appear on video. */
export function renderCaptionPng(text: string, w: number, h: number, look: CaptionLook, strip: number): string {
  const canvas = document.createElement("canvas");
  canvas.width = w;
  canvas.height = strip;
  const ctx = canvas.getContext("2d")!;
  drawCaption(ctx, text, w, h, look, strip);
  return canvas.toDataURL("image/png").split(",")[1];
}
