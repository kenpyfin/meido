# Ingredients compatibility notes

2026-09-13 UTC. These are observed interfaces and a proposed neutral test, not a claim of successful visual validation.

Sources:
- [Ingredients model card](https://huggingface.co/Lightricks/LTX-2.3-22b-IC-LoRA-Ingredients)
- [Official single-stage distilled example](https://github.com/Lightricks/ComfyUI-LTXVideo/blob/master/example_workflows/2.3/LTX-2.3_ICLoRA_Ingredients_Single_Stage_Distilled.json)
- [Core implementation matching installed ComfyUI v0.34.2](https://github.com/Comfy-Org/ComfyUI/blob/v0.34.2/comfy_extras/nodes_lt.py)

The reference is a static video: resize the entire sheet to target resolution, repeat it to the video frame count, encode as an in-context guide, sample, then crop appended reference latents before decoding. The model card calls for descriptions of sheet contents and intended scene, not merely an image filename. A filename alone is not a reference binding mechanism. At initial validation use 768x448, 121 frames, 24fps; distinguish the model card's dev settings from the official distilled example's eight-step schedule.

The installed host exposes core `GetICLoRAParameters` and `LTXVAddGuide`, but not the example's `LTXICLoRALoaderModelOnly` / `LTXAddVideoICLoRAGuide`. The neutral test uses core equivalents with the installed distilled FP8 model; it is an explicit adaptation. Do not add an extra distilled LoRA merely because the dev-checkpoint example uses one.

`ImagePrepForICLora` is a separate side-by-side image/mask preparation utility. Its name does not establish it as the required Ingredients preprocessing. The old runner used a single sheet-plus-scene composite; this differs from the full-length static reference path.

In the matched core source, `LTXVCropGuides` both removes appended guide latents AND clears `keyframe_idxs` and `guide_attention_entries`. Do not describe its output as retaining the previous mid/end/reference conditioning. To retain reference conditioning at a second sampling stage, explicitly reattach valid guides and verify that path. The neutral refinement experiment instead declares a base-model refinement without reference conditioning and saves both stages for comparison.

Separate visual reference appearance from temporal pose constraints. A character/reference sheet is not a frame-zero composition. Adding a first-frame guide is a separate controlled experiment after reference-only conditioning works.
