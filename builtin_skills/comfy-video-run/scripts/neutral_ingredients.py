#!/usr/bin/env python3
"""Build the neutral Ingredients experiment for the installed FP8 LTX stack."""
import argparse
import json
from pathlib import Path

from video_run import validate_spec

REFERENCE = (
    "Reference sheet: The left panel shows an adult man with medium brown skin, short curly dark hair "
    "and a short beard, wearing a navy sweater, beige trousers and white sneakers. The top middle "
    "panel shows his face. The bottom middle panel shows his rust-orange rectangular messenger bag "
    "with one wide dark brown strap and a centered small silver clasp. The right panel shows a bright "
    "studio with pale gray walls, an oak floor and window light from the left. "
)
ACTION = (
    "Generated video: A single continuous full-body fashion shot of this man standing in the bright "
    "studio, wearing the same outfit and the same messenger bag at his left hip, its strap crossing "
    "his chest from his right shoulder. He starts with both arms relaxed at his sides. He slowly "
    "raises his right hand to lightly touch the strap at chest height, pauses, and gives a small "
    "natural closed-mouth smile. His left arm remains relaxed. The bag retains its rectangular shape, "
    "rust-orange fabric, one dark strap and centered silver clasp. The camera remains stationary, "
    "his entire head and both shoes visible throughout with space around his figure. Gentle natural "
    "breathing and soft daylight, realistic skin and knit fabric."
)


def node(kind, **inputs):
    return {"class_type": kind, "inputs": inputs}


def build(sheet, seed=20260913, ingredients=True, first=None, refine=False):
    width, height, frames, fps = 768, 448, 121, 24
    g = {
        "1": node("CheckpointLoaderSimple", ckpt_name="ltx-2.3-22b-distilled-1.1_transformer_only_fp8_scaled.safetensors"),
        "3": node("DualCLIPLoader", clip_name1="gemma_3_12B_it_fp4_mixed.safetensors",
                  clip_name2="ltx-2.3_text_projection_bf16.safetensors", type="ltxv", device="cpu"),
        "4": node("VAELoader", vae_name="LTX23_video_vae_bf16.safetensors"),
        "5": node("CLIPTextEncode", clip=["3", 0], text=(REFERENCE if ingredients else "") + ACTION),
        "6": node("CLIPTextEncode", clip=["3", 0], text="blurry, jittery, distorted, inconsistent motion"),
        "7": node("LTXVConditioning", positive=["5", 0], negative=["6", 0], frame_rate=fps),
        "10": node("EmptyLTXVLatentVideo", width=width, height=height, length=frames, batch_size=1),
        "15": node("LTXVChunkFeedForward", model=["1", 0], chunks=4, dim_threshold=4096),
        "18": node("KSamplerSelect", sampler_name="euler_ancestral_cfg_pp"),
        "19": node("CFGGuider", model=["15", 0], positive=["7", 0], negative=["7", 1], cfg=1.0),
        "20": node("RandomNoise", noise_seed=seed),
        "21s": node("ManualSigmas", sigmas="1.0, 0.99375, 0.9875, 0.98125, 0.975, 0.909375, 0.725, 0.421875, 0.0"),
        "21": node("SamplerCustomAdvanced", noise=["20", 0], guider=["19", 0], sampler=["18", 0],
                   sigmas=["21s", 0], latent_image=["10", 0]),
        "22": node("VAEDecodeTiled", samples=["21", 0], vae=["4", 0], tile_size=512, overlap=64,
                   temporal_size=64, temporal_overlap=8),
        "23": node("CreateVideo", images=["22", 0], fps=fps),
        "24": node("SaveVideo", video=["23", 0], filename_prefix="neutral-stage1", format="mp4",
                   codec="h264", encoding="re-encode", crf=12.0),
    }
    assets = {}
    if first:
        assets["8"] = str(Path(first).resolve())
        g["8"] = node("LoadImage", image="assigned-at-upload.png")
        g["8s"] = node("ImageScale", image=["8", 0], width=width, height=height,
                       upscale_method="lanczos", crop="disabled")
        g["10"] = node("LTXVImgToVideo", positive=["7", 0], negative=["7", 1], vae=["4", 0],
                       image=["8s", 0], width=width, height=height, length=frames, batch_size=1, strength=1.0)
        g["21"]["inputs"]["latent_image"] = ["10", 2]
        g["19"]["inputs"].update(positive=["10", 0], negative=["10", 1])
    if ingredients:
        assets["50"] = str(Path(sheet).resolve())
        g["1ic"] = node("LoraLoaderModelOnly", model=["1", 0],
                         lora_name="ltx-2.3-22b-ic-lora-ingredients-0.9.safetensors", strength_model=1.0)
        g["1icp"] = node("GetICLoRAParameters", iclora_model=["1ic", 0])
        g["15"]["inputs"]["model"] = ["1ic", 0]
        g["50"] = node("LoadImage", image="assigned-at-upload.png")
        g["50s"] = node("ImageScale", image=["50", 0], width=width, height=height,
                        upscale_method="bilinear", crop="disabled")
        g["50b"] = node("RepeatImageBatch", image=["50s", 0], amount=frames)
        g["50g"] = node("LTXVAddGuide", positive=["10", 0] if first else ["7", 0],
                        negative=["10", 1] if first else ["7", 1], vae=["4", 0],
                        latent=["10", 2] if first else ["10", 0], image=["50b", 0],
                        frame_idx=0, strength=1.0, iclora_parameters=["1icp", 0])
        g["19"]["inputs"].update(positive=["50g", 0], negative=["50g", 1])
        g["21"]["inputs"]["latent_image"] = ["50g", 2]
        g["21c"] = node("LTXVCropGuides", positive=["50g", 0], negative=["50g", 1], latent=["21", 0])
        g["22"]["inputs"]["samples"] = ["21c", 2]
    outputs = {"24": {"width": width, "height": height, "frames": frames, "fps": fps}}
    if refine:
        # Save Stage-1 as well. Refinement uses the base model and plain text conditioning;
        # CropGuides clears guide metadata, so this stage is explicitly not reference-locked.
        g["30"] = node("LatentUpscaleModelLoader", model_name="ltx-2.3-spatial-upscaler-x2-1.1.safetensors")
        g["31"] = node("LTXVLatentUpsampler", samples=["21c", 2] if ingredients else ["21", 0],
                       upscale_model=["30", 0], vae=["4", 0])
        g["32"] = node("LTXVChunkFeedForward", model=["1", 0], chunks=4, dim_threshold=4096)
        g["35"] = node("ManualSigmas", sigmas="0.85, 0.7250, 0.4219, 0.0")
        g["36"] = node("CFGGuider", model=["32", 0], positive=["7", 0], negative=["7", 1], cfg=1.0)
        g["37"] = node("RandomNoise", noise_seed=42)
        g["38"] = node("SamplerCustomAdvanced", noise=["37", 0], guider=["36", 0], sampler=["18", 0],
                       sigmas=["35", 0], latent_image=["31", 0])
        g["42"] = node("VAEDecodeTiled", samples=["38", 0], vae=["4", 0], tile_size=512, overlap=64,
                       temporal_size=64, temporal_overlap=8)
        g["43"] = node("CreateVideo", images=["42", 0], fps=fps)
        g["44"] = node("SaveVideo", video=["43", 0], filename_prefix="neutral-refined", format="mp4",
                       codec="h264", encoding="re-encode", crf=12.0)
        outputs["44"] = {"width": width * 2, "height": height * 2, "frames": frames, "fps": fps}
    spec = {"graph": g, "assets": assets, "outputs": outputs,
            "hypothesis": "A neutral character and distinctive bag remain consistent without sheet leakage",
            "changed_factor": "initial reference-path validation", "baseline": None,
            "adaptations": ["Installed distilled transformer-only FP8 checkpoint, not official dev checkpoint plus distilled LoRA",
                            "Core LTXVAddGuide + GetICLoRAParameters in place of unavailable Lightricks custom IC nodes",
                            "Video only, no audio branch; ChunkFeedForward for 16GB GPU"],
            "refinement": "base model; no claim of retained reference conditioning" if refine else "off"}
    validate_spec(spec)
    return spec


if __name__ == "__main__":
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--sheet", required=True)
    p.add_argument("--out", type=Path, required=True)
    p.add_argument("--seed", type=int, default=20260913)
    p.add_argument("--no-ingredients", action="store_true")
    p.add_argument("--first")
    p.add_argument("--refine", action="store_true")
    a = p.parse_args()
    a.out.write_text(json.dumps(build(a.sheet, a.seed, not a.no_ingredients, a.first, a.refine), indent=2) + "\n")
    print(a.out.resolve())
