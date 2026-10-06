/**
 * The single preview <video> element, shared so the timeline and transcript can seek
 * without threading refs through the component tree.
 */
let video: HTMLVideoElement | null = null;

export const player = {
  attach(el: HTMLVideoElement | null) {
    video = el;
  },
  get el() {
    return video;
  },
  seek(t: number) {
    if (!video) return;
    const d = Number.isFinite(video.duration) ? video.duration : Infinity;
    video.currentTime = Math.min(Math.max(0, t), d);
  },
  toggle() {
    if (!video) return;
    if (video.paused) void video.play();
    else video.pause();
  },
  pause() {
    video?.pause();
  },
  nudge(delta: number) {
    if (video) player.seek(video.currentTime + delta);
  },
};
