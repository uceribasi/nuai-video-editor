"""nuai voice worker.

Long-lived process the app talks to over stdin/stdout, one JSON object per line.
Requests carry an "id"; the worker answers with progress events and one final
{"id", "result"} or {"id", "error"}. Models stay loaded between requests.

Commands:
  ping
  separate  {input, out_dir}                    -> {vocals, background, sample_rate}
  clone     {model_dir, ref_audio, ref_text, fast, items: [{text, language, duration, out}]}
                                                -> {items: [{out, seconds}], sample_rate}
"""

import json
import os
import sys
import traceback

# transformers moves weights to the GPU from several threads at once; PyTorch's MPS
# kernel cache isn't thread-safe and that randomly crashes or hangs the load.
# Load sequentially instead (must be set before transformers is imported).
os.environ.setdefault("HF_DEACTIVATE_ASYNC_LOAD", "1")

# Everything the app reads comes through stdout; keep libraries from writing to it.
_out = sys.stdout
sys.stdout = sys.stderr

state = {"tts": None, "tts_key": None, "prompt": None, "prompt_key": None, "demucs": None}


def send(obj):
    _out.write(json.dumps(obj, ensure_ascii=False) + "\n")
    _out.flush()


def device():
    import torch

    if torch.backends.mps.is_available():
        return "mps"
    if torch.cuda.is_available():
        return "cuda"
    return "cpu"


def load_tts(model_dir, fast):
    key = (model_dir, fast)
    if state["tts_key"] != key:
        import torch
        from omnivoice import OmniVoice

        dev = device()
        dtype = torch.float16 if fast and dev != "cpu" else torch.float32
        state.update(tts=None, prompt=None, prompt_key=None)
        state["tts"] = OmniVoice.from_pretrained(model_dir, device_map=dev, dtype=dtype)
        state["tts_key"] = key
    return state["tts"]


def clone(req):
    import numpy as np
    import soundfile as sf

    fast = bool(req.get("fast", True))
    model = load_tts(req["model_dir"], fast)
    sr = getattr(model, "sampling_rate", None) or getattr(model, "sr", 24000)
    steps = 16 if fast else 32

    # The reference is encoded once and reused for every line.
    pkey = (req["ref_audio"], req["ref_text"])
    if state["prompt_key"] != pkey:
        state["prompt"] = model.create_voice_clone_prompt(ref_audio=req["ref_audio"], ref_text=req["ref_text"])
        state["prompt_key"] = pkey

    items = req["items"]
    results = []
    batch = max(1, int(req.get("batch", 4)))
    for start in range(0, len(items), batch):
        chunk = items[start : start + batch]
        send({"id": req["id"], "event": "progress", "progress": start / len(items)})
        audios = model.generate(
            text=[it["text"] for it in chunk],
            language=[it.get("language") for it in chunk],
            voice_clone_prompt=[state["prompt"]] * len(chunk),
            duration=[it.get("duration") for it in chunk],
            num_step=steps,
        )
        for it, audio in zip(chunk, audios):
            a = np.asarray(audio, dtype=np.float32).squeeze()
            sf.write(it["out"], a, sr)
            results.append({"out": it["out"], "seconds": round(len(a) / sr, 3)})
    send({"id": req["id"], "event": "progress", "progress": 1.0})
    return {"items": results, "sample_rate": sr}


def separate(req):
    import soundfile as sf
    import torch
    from demucs.apply import apply_model
    from demucs.pretrained import get_model

    if state["demucs"] is None:
        state["demucs"] = get_model("htdemucs")
        state["demucs"].eval()
    model = state["demucs"]

    data, sr = sf.read(req["input"], always_2d=True, dtype="float32")
    wav = torch.from_numpy(data.T.copy())
    if sr != model.samplerate:
        import torchaudio.functional as F

        wav = F.resample(wav, sr, model.samplerate)
    if wav.shape[0] == 1:
        wav = wav.repeat(model.audio_channels, 1)
    ref = wav.mean(0)
    mean, std = ref.mean(), ref.std() + 1e-8
    wav = (wav - mean) / std

    length = wav.shape[-1]

    def on_chunk(info):
        # Demucs reports each chunk's start offset in samples.
        if info.get("state") == "end":
            send({"id": req["id"], "event": "progress", "progress": min(0.99, info.get("segment_offset", 0) / length)})

    sources = apply_model(model, wav[None], device=device(), split=True, overlap=0.25, progress=False, callback=on_chunk)[0]
    sources = sources * std + mean
    vocals = sources[model.sources.index("vocals")]
    background = sources.sum(0) - vocals

    os.makedirs(req["out_dir"], exist_ok=True)
    vpath = os.path.join(req["out_dir"], "vocals.wav")
    bpath = os.path.join(req["out_dir"], "background.wav")
    sf.write(vpath, vocals.T.cpu().numpy(), model.samplerate)
    sf.write(bpath, background.T.cpu().numpy(), model.samplerate)
    return {"vocals": vpath, "background": bpath, "sample_rate": model.samplerate}


HANDLERS = {"ping": lambda req: {"device": device()}, "clone": clone, "separate": separate}


def main():
    send({"event": "ready"})
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        req = {}
        try:
            req = json.loads(line)
            result = HANDLERS[req["cmd"]](req)
            send({"id": req.get("id"), "result": result})
        except Exception as e:  # report and keep serving
            traceback.print_exc()
            send({"id": req.get("id"), "error": f"{type(e).__name__}: {e}"})


if __name__ == "__main__":
    main()
