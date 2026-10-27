# Sketch Assist backends: capability and model matrix

Issue [#76](https://github.com/HenryCorn/WaltCut/issues/76), epic [#75](https://github.com/HenryCorn/WaltCut/issues/75).
Matrix version **v1, researched 2026-10-26**. Read-only desk research: nothing was uploaded, no paid or
keyed API was called, no weights or runtimes were downloaded, nothing was run.

## How to read this

- **S** = supported, per the primary page cited. **U** = documented as unsupported or no field exists on the
  documented schema. **UNTESTED** = a documented field or feature exists but nobody has exercised it for
  WaltCut's needs (this applies to every quality, drift and "follows a loose sketch" claim). **ND** = the
  page read does not say.
- Memory and latency figures are **documented** (a cited page states them, for the hardware and date it
  states) or **estimated** (my arithmetic from documented sizes). **Nothing here is measured.**
- Prices are fal.ai list prices **checked 2026-10-26** on the model pages and https://fal.ai/pricing. They change.
- Owner context: Apple M4 Pro, **24 GB unified memory** (confirmed by the owner and `sysctl hw.memsize` = 25769803776),
  personal non-commercial use, about $5 for the first proof, nothing authorised yet.
- "Identical character across poses" is a goal no backend promises. A fixed seed is not evidence of identity.
- Today's code (`crates/genai`): `Restyler::restyle(&Picture, &RestyleRequest) -> Vec<u8>`; the request carries
  prompt, negative prompt, strength and seed only; the only provider is fal.ai via a **synchronous**
  `POST https://fal.run/<endpoint>` with `image_url` (data URI). It has no mask, no second image, no sketch/reference
  role and no cancel. Nothing in this document claims it does.

## 1. Shortlist

| # | Route | Where | Role in Sketch Assist |
|---|---|---|---|
| R1 | fal `fal-ai/flux-general` (FLUX.1 dev + ControlNet / ControlNet-Union / IP-Adapter / masks) | cloud | one call that can take sketch + style reference + region |
| R2 | fal `fal-ai/flux-pro/v1/fill` | cloud | regional edit (image + mask) |
| R3 | fal `fal-ai/flux-pro/kontext` (and `.../kontext/max/multi`) | cloud | instruction edit from a reference image; pose/character from an approved drawing |
| R4 | fal `fal-ai/nano-banana/edit` | cloud | multi-image reference edit |
| R5 | fal `fal-ai/ideogram/character` | cloud | character reference (+ reference mask) |
| R6 | mflux (MLX) running FLUX.1 Canny / Fill / Kontext / Redux | local | everything R1-R3 do, on the Mac |
| R7 | SDXL + T2I-Adapter sketch + IP-Adapter (+ inpaint), via diffusers/MPS or ComfyUI server | local | lighter local stack |
| R8 | Runtimes assessed and not shortlisted: Core ML (`apple/ml-stable-diffusion`), stable-diffusion.cpp, Draw Things | local | see section 4 |

## 2. Cloud routes (fal.ai)

Common platform facts ([queue docs](https://fal.ai/docs/model-apis/model-endpoints/queue), read 2026-10-26):
queue base `https://queue.fal.run`; submit `POST /{owner}/{app}/{path}` returns `request_id`, `status_url`,
`cancel_url`; status `GET .../requests/{id}/status`; result `GET .../requests/{id}`; **cancel
`PUT .../requests/{id}/cancel`** (200 removed from queue and never runs; 202 `CANCELLATION_REQUESTED`, "may still
complete"; 400 already completed; 404 not found). Header `Authorization: Key $FAL_KEY`. Today's synchronous
`fal.run` call has no cancel; the docs say it "does not retry" and recommend queue/subscribe for production
([model-endpoints docs](https://fal.ai/docs/model-apis/model-endpoints)).
Privacy ([FAQ](https://fal.ai/docs/model-apis/faq)): generated media is on the fal CDN "at least 7 days by default", URLs
are publicly readable by anyone with the link unless restricted; `sync_mode: true` on the model schemas returns a data
URI and "output isn't saved to request history". The FAQ page does **not** say whether fal trains on inputs, so that is
**ND**: read https://fal.ai/privacy before any upload. Concurrency: new accounts start at 2 simultaneous requests.
Data-URI inputs: today's `fal.rs` sends them; the docs pages read did not cover it (ND, UNTESTED per endpoint).

| Capability | R1 flux-general | R2 flux-pro fill | R3 kontext (pro) | R4 nano-banana edit | R5 ideogram character |
|---|---|---|---|---|---|
| Sketch constraint | `controlnets` (1 max) and `controlnet_unions` (1 max; modes canny, tile, depth, blur, pose, gray, low-quality; **no scribble/lineart mode listed**), `control_loras` (canny, depth). Following a loose ink sketch: UNTESTED | U (no control image) | U as a control; the sketch can only be the `image_url` to be edited. UNTESTED | U as a control; sketch only as an input image. UNTESTED | U |
| Style reference | `ip_adapters` (needs `path`, `image_encoder_path`, `image_url`, `scale`) and `reference_image_url` + `reference_strength` (0.65 default). UNTESTED | U | via the single `image_url` / `image_urls`. UNTESTED | `image_urls` list (count limit ND). UNTESTED | `image_urls` = style references, 10 MB total |
| Identity / character reference | IP-Adapter / EasyControl `subject` mode, you supply weights paths. UNTESTED | U | Model card says it can use "character, style, or object references" and stays consistent over successive edits ([HF card](https://huggingface.co/black-forest-labs/FLUX.1-Kontext-dev)). UNTESTED for a dog | multi-image input. UNTESTED | `reference_image_urls` "Currently only 1 image is supported"; 10 MB; FICTION style option. UNTESTED |
| Regional edit (mask) | `mask_image_url` inside controlnet / ip-adapter entries and `fill_image.fill_image_url`; there is **no documented plain inpaint field** and no top-level `image_url` or `strength`. UNTESTED | **S**: `image_url` + `mask_url`, "dimensions must match" | **U** (no mask) | **U** (no mask) | `reference_mask_urls` masks the **character reference**, not an edit region; 1 mask |
| Max size / alpha | no limits stated (default 512; `image_size` enum or width/height); output png/jpeg; alpha ND (assume none) | none stated; image and mask must match; jpeg/png; alpha ND | `aspect_ratio` enum only, no limit stated; alpha ND | `aspect_ratio` enum; alpha ND | `image_size` enum or custom; alpha ND |
| Cancel | queue cancel (PUT) S; sync none | same | same | same | same |
| Privacy | cloud upload; public CDN URLs; training use ND | same | same | same | same |
| Price (2026-10-26) | **$0.075 per megapixel**, rounded up per MP ([page](https://fal.ai/models/fal-ai/flux-general)) | **$0.05 per MP** ([page](https://fal.ai/models/fal-ai/flux-pro/v1/fill)) | **$0.04 per image** ([page](https://fal.ai/models/fal-ai/flux-pro/kontext)); Fill page lists Kontext [pro] $0.055/MP and [max] $0.11/MP, which disagrees: re-check at run time | **$0.0398 per image** ([page](https://fal.ai/models/fal-ai/nano-banana/edit)) | not stated on the page |
| Latency | ND (UNTESTED) | ND | ND | ND | ND |
| Licence / weights | page tag "Commercial use"; underlying FLUX.1 [dev] weights are under the FLUX.1 [dev] Non-Commercial License (flagged; personal use here); IP-Adapter / ControlNet weights are third party | "Commercial use permitted via fal partnership" (BFL partner) | partner model, "Commercial use" tag | partner (Google) model; terms ND | partner; terms ND |
| Maintenance | active (pages current) | active | active | active | active |
| Weights | hosted, not downloadable by us | hosted | hosted | hosted | hosted |

Real request examples (copied from the cited API pages, abbreviated only where noted):

R1 `fal-ai/flux-canny` sibling `fal-ai/flux-lora-canny` ([API page](https://fal.ai/models/fal-ai/flux-lora-canny/api), $0.035/MP, FLUX [dev] control LoRA; the canny map is derived from the image):
```json
{ "prompt": "A blue owl.", "num_inference_steps": 28, "guidance_scale": 3.5, "num_images": 1,
  "enable_safety_checker": true, "output_format": "jpeg", "acceleration": "none",
  "image_url": "https://storage.googleapis.com/falserverless/example_inputs/dog.png" }
```
R1 `flux-general`: the page's own JSON example is text-only. The control, IP-Adapter and mask fields exist in its schema
([API page](https://fal.ai/models/fal-ai/flux-general/api)); **no official example combines them, so a sketch + reference + mask call is UNTESTED and would be built from the schema.** Shape from the schema (field names as listed there; values are placeholders):
```json
{ "prompt": "...", "image_size": {"width": 1024, "height": 1024}, "output_format": "png",
  "controlnet_unions": [{"path": "<hf repo>", "controls": [{"control_image_url": "...", "control_mode": "canny", "conditioning_scale": 1}]}],
  "ip_adapters": [{"path": "<hf repo>", "image_encoder_path": "<hf repo>", "image_url": "...", "scale": 0.6, "mask_image_url": "..."}] }
```
R2 ([API page](https://fal.ai/models/fal-ai/flux-pro/v1/fill/api)):
```json
{ "prompt": "A knight in shining armour holding a greatshield with \"FAL\" on it", "num_images": 1,
  "output_format": "jpeg", "safety_tolerance": "2",
  "image_url": "https://storage.googleapis.com/falserverless/flux-lora/example-images/knight.jpeg",
  "mask_url": "https://storage.googleapis.com/falserverless/flux-lora/example-images/mask_knight.jpeg" }
```
R3 ([API page](https://fal.ai/models/fal-ai/flux-pro/kontext/api)):
```json
{ "prompt": "Put a donut next to the flour.", "guidance_scale": 3.5, "num_images": 1,
  "output_format": "jpeg", "safety_tolerance": "2",
  "image_url": "https://v3.fal.media/files/rabbit/rmgBxhwGYb2d3pl3x9sKf_output.png" }
```
R4 ([API page](https://fal.ai/models/fal-ai/nano-banana/edit/api)):
```json
{ "prompt": "make a photo of the man driving the car down the california coastline", "num_images": 1,
  "aspect_ratio": "auto", "output_format": "png", "safety_tolerance": "4", "limit_generations": true,
  "image_urls": ["https://storage.googleapis.com/falserverless/example_inputs/nano-banana-edit-input.png",
                 "https://storage.googleapis.com/falserverless/example_inputs/nano-banana-edit-input-2.png"] }
```
R5 ([API page](https://fal.ai/models/fal-ai/ideogram/character/api)), JSON fields from the page: `prompt`, `reference_image_urls` (1 used), optional
`image_urls` (style refs), `reference_mask_urls`, `rendering_speed` (`TURBO`/`BALANCED`/`QUALITY`), `style` (`AUTO`/`REALISTIC`/`FICTION`),
`image_size`, `seed`, `expand_prompt`.

Cloud summary: **no single fal endpoint documents all of (loose ink-sketch constraint) + (pinned character reference) + (edit region)**.
R1 is the only one whose schema has all three kinds of field, but its behaviour is UNTESTED and its documentation is thin (inpaint is not a
plain documented mode). R2 is the only one with a clean documented mask. R3/R4/R5 take references but no mask.

## 3. Local routes on the 24 GB M4 Pro

Headroom estimate (**estimated**): macOS plus a running WaltCut (egui, decoded frames, cels) plausibly leave about 16 to 18 GB
for models before the system compresses or swaps; budget about 14 GB to stay comfortable. diffusers states Apple-silicon
performance "is very sensitive to memory pressure", swapping "significantly degrades performance"
([diffusers MPS docs](https://huggingface.co/docs/diffusers/optimization/mps), read 2026-10-26).

| Capability | R6 mflux (MLX), FLUX.1 | R7 SDXL + T2I-Adapter sketch + IP-Adapter (+ inpaint) |
|---|---|---|
| Runtime / Apple silicon | MLX, Python (`requires-python >= 3.10`), [repo](https://github.com/filipstrand/mflux), MIT. S on Apple silicon (MLX project; float16 note for M1-family) | diffusers `mps` device is S ([docs](https://huggingface.co/docs/diffusers/optimization/mps)); ComfyUI documents Apple silicon install via PyTorch nightly, desktop app recommended ([repo](https://github.com/comfyanonymous/ComfyUI)) |
| Sketch constraint | **Canny ControlNet only** (InstantX FLUX.1-dev Canny, trained for dev), `--controlnet-image-path`, strength "sensitive", start ~0.4. Lineart/scribble ND. Ink-sketch following UNTESTED | T2I-Adapter-SDXL **sketch**: Apache-2.0, ~79M params, input "white outlines on a black background", example `adapter_conditioning_scale=0.9`, 30 steps, via `StableDiffusionXLAdapterPipeline` ([card](https://huggingface.co/TencentARC/t2i-adapter-sketch-sdxl-1.0)). Fits a drawing better than canny. Following a loose sketch: UNTESTED. SD1.5 scribble ControlNet also exists (OpenRAIL, ~0.4B, [card](https://huggingface.co/lllyasviel/control_v11p_sd15_scribble)) |
| Style / identity reference | **Redux**: image embedding combined with text, `--redux-image-strengths 0..1`; page warns "the reference image [tends] to dominate". Kontext uses a single input image. Identity of a dog: UNTESTED | IP-Adapter SDXL variants (`ip-adapter_sdxl`, `_vit-h`, `plus_sdxl_vit-h`, face variants), Apache-2.0 ([card](https://huggingface.co/h94/IP-Adapter)). Character identity: UNTESTED, and IP-Adapter is a global-appearance cue, not an identity lock (my reading; no source claims identity) |
| Regional edit | **Fill**: `mflux-generate-fill --image-path --masked-image-path`; "White areas in the mask will be regenerated, while black areas will be preserved"; guidance ~30, 20-30 steps; **S** | SDXL inpainting checkpoints/pipelines exist but I did **not** read a card for this matrix: UNTESTED / not read |
| Max size / alpha | Kontext "standard resolutions (1024x1024, 768x1024)"; IC-Edit forces 512 px width; alpha ND (assume none) | SDXL native 1024; diffusers warns `mps` fails on NDArrays larger than 2^32 elements; alpha ND |
| Cancel | CLI process: kill it (S by OS). Per-step progress ND | ComfyUI: `POST /interrupt`, `GET /ws` progress messages documented ([routes](https://docs.comfy.org/development/comfyui-server/comms_routes)); diffusers: kill the sidecar |
| Privacy | all local; gated weights need a Hugging Face account to download | all local |
| Memory | **estimated**: 12B parameters at 8-bit (`-q 8` is used throughout the README) is roughly 12 GB of weights plus text encoders and activations: tight next to WaltCut; at bf16 (24 GB) it does **not** fit. No RAM figure documented. Downloads documented: Fill 33.92 GB, Kontext ~34 GB, ControlNet ~3.58 GB, Redux 1.1 GB | **estimated**: SDXL 3B parameters ([card](https://huggingface.co/stabilityai/stable-diffusion-xl-base-1.0)) at fp16 is about 6 to 7 GB, plus adapter (79M) and IP-Adapter image encoder (ViT-H 632M, documented parameter count) roughly 1.5 to 2 GB at fp16: about 9 to 10 GB total, which fits with headroom. Apple documents SDXL 4.5-bit palettized Core ML at ~1.46 GB vs 5.2 GB ([repo](https://github.com/apple/ml-stable-diffusion)) |
| Latency | ND; no M4 numbers anywhere I read. **UNTESTED** | Only documented figure: Core ML SDXL 1024x1024 on M2 Max 37 s, M1 Max 46 s (Apple, July 2023, macOS 14 beta); M4 Pro and diffusers/MPS: **ND**. Estimated 20 to 60 s per 1024 image: not measured, within the owner's 60 s tolerance only at the low end |
| Licence / weights | mflux MIT. FLUX.1 [dev] / Fill-dev / Kontext-dev weights: **FLUX.1 [dev] Non-Commercial License**, gated; the Kontext card says outputs "can be used for personal, scientific, and commercial purposes" ([card](https://huggingface.co/black-forest-labs/FLUX.1-Kontext-dev)); personal use is fine, record it | SDXL: CreativeML Open RAIL++-M (card says "intended for research purposes only" in its use section; terms not read in full: read before download); T2I-Adapter and IP-Adapter Apache-2.0; ControlNet SD1.5 scribble OpenRAIL; ComfyUI is **GPL-3.0** |
| Maintenance | active (772 commits, org move noted Aug 2026; no releases listed) | diffusers and ComfyUI active (ComfyUI states a major core release roughly every two weeks); SDXL-era checkpoints are old but stable |
| Rust shape | spawn the `mflux-*` CLI as a subprocess, or a user-run sidecar; per-call model load cost ND | user-run **local HTTP service** (ComfyUI `POST /prompt`, `/upload/image`, `/view`, `/interrupt`, `/ws`) or a small Python sidecar; WaltCut stays pure Rust and bundles nothing GPL. Whether a user-installed GPL server reached over localhost fits the CLAUDE.md "never ship GPL" rule is a policy call for the owner (it is not copied or linked) |

Realism for **sketch + reference + inpaint at WaltCut sizes (about 1024 px) on 24 GB**:
- **R7 (SDXL class): memory-realistic (estimated).** Quality, identity and latency are UNTESTED. Needs a Python stack and about 10 to 15 GB of downloads (estimated) and a licence read. A vision model run sequentially (unloaded first) does not change this.
- **R6 (FLUX class): not realistic as the first route.** Each of Fill and Kontext is a separate ~34 GB download (documented), 8-bit weights alone are estimated at about 12 GB, there is no documented M4 latency, and a combined sketch + reference + mask pass needs several of these models in turn (ControlNet, Redux, Fill).
- Concurrent SDXL + ControlNet/adapter + IP-Adapter + a 4-7B vision model is **not** a safe plan on 24 GB (estimated 9-10 GB + ~5-6 GB with context, plus macOS and WaltCut). Run the vision model first, then unload it (Ollama `keep_alive: 0` unloads, [API docs](https://raw.githubusercontent.com/ollama/ollama/main/docs/api.md)).

### Local runtime notes (R8, assessed, not shortlisted)

- **Core ML `apple/ml-stable-diffusion`** (MIT): SD 1.x/2.x/SDXL/SD3 supported; **ControlNet "Not yet supported for SDXL"** (SD only); mixed-bit palettization documented; only Swift/Python, no HTTP service. Fine for SD1.5-class; no SDXL control.
- **stable-diffusion.cpp** (MIT, [repo](https://github.com/leejet/stable-diffusion.cpp)): Metal backend S; models include SDXL, FLUX.1 incl. **Kontext-dev**, Qwen Image Edit; ControlNet listed "with SD 1.5"; IP-Adapter "SD 1.5 and SDXL, including Plus"; PhotoMaker. Inpainting and an HTTP server API are **not documented on the pages I could read** (`start-server.sh` exists, news mentions an embedded web UI). C++ library or subprocess; the CLI README I read lacked the control/mask flags. Promising and MIT but under-documented: UNTESTED / ND.
- **Draw Things** (community repo GPL-3.0): README documents a gRPC server and CLI; **an A1111-style HTTP API is only mentioned by third-party wrappers** in search results, with no ControlNet or inpaint documentation found. Not documented in primary sources: ND.
- **ComfyUI** as a server is the best-documented HTTP boundary (routes above), but its `POST /prompt` payload format is not defined on the docs page I read.

## 4. The olwal/ai-drawing stack on Apple silicon (verified from its README and source)

| Piece | Evidence | Runs on Apple silicon? |
|---|---|---|
| App (Cinder C++, OpenGL) | README: Visual Studio 2022 solution only (`vc2022/`), WinHTTP client | No: Windows build only; Cinder itself is cross-platform but the project ships only a VS solution |
| Ollama LLaVA interpretation | README: `ollama pull llava:7b`; Ollama has macOS installers ([repo](https://github.com/ollama/ollama), MIT) | **Yes** for Ollama itself (macOS install documented; the page read does not state Metal explicitly) |
| StreamDiffusionSpoutService (generation) | [README](https://github.com/olwal/streamdiffusion-spout-service): "Windows only (requires Spout)", NVIDIA CUDA, default `stabilityai/sd-turbo`, 512x512, TensorRT/xformers | **No** |
| StreamDiffusion library | [README](https://github.com/cumulo-autumn/StreamDiffusion): CUDA 11.8/12.1 PyTorch, benchmark on RTX 4090 / Ubuntu; macOS, MPS not mentioned | **Not documented** (assume no) |
| Spout | Windows GPU texture sharing | No (macOS equivalent is Syphon; not needed) |

So only the vision half transfers; the generation half does not. Repo facts: MIT, last push 2025-12-20, 35 stars (GitHub API).

## 5. Piece-by-piece review of olwal/ai-drawing (read-only; reimplement independently in Rust, copy no code)

Source read via the GitHub API: `src/AiDrawingApp.cpp`, `VectorDrawing.cpp`, `DrawingApp.cpp`, README. MIT licence; WaltCut copies nothing.

| Piece | What it does | In WaltCut already? | Apple silicon? | Recommendation |
|---|---|---|---|---|
| Vision interpretation (Ollama `llava:7b`) | Sends the canvas image with the prompt "provide a concise, but creative description of what is being drawn, no more than 10 words" and forwards the answer as the diffusion prompt; `injectionPrompt` can append a style suffix | No | Yes (Ollama) | **Reuse the idea**, changed: one explicit, editable suggestion, not continuous (see section 6) |
| Semantic averaging of results | Re-prompts the LLM with the last results (weighted, recent first) to produce a stable description, plus a "mechanical check" to reduce jitter | No | Yes | **Reuse the idea** only as "pin the character caption once and reuse it", which also helps same-character; skip the continuous averaging loop |
| Continuous augmentation | Streams the canvas to StreamDiffusion, shows result live | No | No (Windows/CUDA) | **Skip**: #75 rules out continuous generation; use explicit Enhance |
| Compositing | Draws the generated texture under or behind the vector drawing in the same window | partially (layers, cels) | n/a | **Replace**: keep original strokes untouched on their own layer, put generated raster on a separate layer with provenance; hard-composite approved pixels back after any model call |
| Stroke smoothing | Triangular-weight moving average over a window of raw points (`smoothing` level) | Yes per the owner (smoothing, rope stabilizer) | n/a | **Skip** |
| Dynamic width | Speed maps to width (faster, thinner) times pressure | Yes per the owner (pressure curves) | n/a | **Skip** (maybe check speed-to-width exists) |
| Undo/redo command pattern | `AddStrokeCommand` executed through a command stack | Yes (`edit.undo`, command system) | n/a | **Skip** |
| Canvas FBO, colour/width keys, save PNG | OpenGL FBO canvas, key bindings, `Ctrl+Space` saves PNG | Yes (cel raster, export) | n/a | **Skip** |
| ThreadSafeList of AI results | Locked list handing async results to the UI | Yes (jobs) | n/a | **Skip** (WaltCut uses jobs under `catch_unwind`) |
| OSC | UDP messages to TouchDesigner/StreamDiffusion (`/prompt`, `/trigger`) | No | Yes (protocol) | **Skip**: WaltCut's control channel and providers replace it |
| Spout in/out | Shares textures with the diffusion service | No | No | **Replace** with a plain request/response provider boundary |

Not verified from my reads: how `AiDrawingApp` timing paces requests (I only grepped the source); the LLaVA, Qwen VL and moondream licences (the Ollama pages did not state them); `stabilityai/sd-turbo` licence (not read).

## 6. Vision interpretation as a way to remove prompt typing

What it buys: a caption such as "a brown dog walking" so the user need not type. What it does not do: it cannot express **style** ("storybook, flat cartoon, ink line, cut-out paper"), nor lock identity. For the owner's goals the useful design is a **user-visible, editable suggested caption, generated on demand and pinned per character**, with a fixed style suffix stored as a preset. This is a text aid only; it is **not** a substitute for sketch, reference or mask control.

Ollama-library vision models (download sizes **documented** on their library pages, read 2026-10-26; runtime memory is **estimated** as download size plus roughly 1 to 2 GB for context and runtime, never measured):

| Model | Download (documented) | Fits 24 GB beside an SDXL stack? | Time cost |
|---|---|---|---|
| `moondream:1.8b` | 1.7 GB | yes, even concurrently (estimated) | UNTESTED; small model, expected a few seconds (estimated) |
| `qwen2.5vl:3b` | 3.2 GB | yes if sequential or concurrent with SDXL ~10 GB (estimated) | UNTESTED |
| `gemma3:4b` (vision) | 3.4 GB; Gemma Terms of Use | yes (estimated) | UNTESTED |
| `llava:7b` (olwal default) | 4.7 GB | sequential preferred (estimated) | UNTESTED |
| `qwen2.5vl:7b` | 6.0 GB | sequential only (estimated) | UNTESTED |
| `llava:13b` | 8.0 GB | not alongside an image model | UNTESTED |

Time cost: no latency figure for these on Apple silicon was found in the primary docs; model load adds time unless kept warm
(`keep_alive` default 5 minutes). Assume a few to tens of seconds per caption (estimated) and measure it in the proof. Ollama's API docs
describe no explicit cancel for an in-flight generation (UNTESTED whether closing the connection stops it). Image bytes go only to
`localhost:11434` (local); this is the only vision step with no upload. How well any of these captions a rough walking-dog sketch is **UNTESTED** and
should be one line of the human proof. Recommendation: a vision step is an **optional enabler**, ship it after the first route works, and
default to the user typing/editing a short caption if the local vision step is absent.

## 7. Recommendation for SA0

**Principles:** no silent substitution. If a request needs a sketch constraint, reference or mask and the provider lacks it, return an
explicit `Unsupported` that names the missing capability; do not fall back to whole-image img2img or a text-only style prompt.
Independently of any model, WaltCut should **composite the approved pixels back** over a returned image outside the edit region
(deterministic, local); this guarantees "nothing approved changes" in the output file, but it is a safeguard, not proof that a model
can edit a region coherently.

| Decision | Choice |
|---|---|
| First route for the #77 proof | **Cloud, fal.ai (the owner has a key): compare R1 `flux-general` (all-in-one, risky), R2 `flux-pro/v1/fill` (documented mask) and R3 `kontext` (pinned reference), with R4 `nano-banana/edit` as a cheap control.** All use the existing provider boundary. |
| Minimal implementation shape (after #27) | Extend `crates/genai` with a capability descriptor per endpoint (`sketch`, `style_ref`, `identity_ref`, `mask`, `alpha`, `cancel`, `max_px`), a request type that carries role-tagged images and an optional mask, and the fal **queue** API (submit, status, cancel via PUT) instead of only synchronous `fal.run`. Fake-provider tests: capability selection and `Unsupported`. No new provider framework. |
| Documented fallback | R2 + R3 as a two-step cloud recipe (Kontext for pose/character, Fill with a mask for additive edits) if R1's undocumented combination fails. |
| Local route | **Deferred, not substituted.** R7 (SDXL + T2I-Adapter sketch + IP-Adapter, user-run local HTTP service) is the only memory-realistic local candidate on 24 GB (estimated); R6 FLUX-class is not realistic as a first route. Revisit under #1 only after the cloud proof shows which capabilities matter; needs its own consent for downloads (~10 to 15 GB, estimated) and a licence read. |
| Vision interpretation | Optional enabler after the first route; Ollama `qwen2.5vl:3b` / `gemma3:4b` / `moondream` candidates, sequential with the image model. |
| olwal reuse | Idea only: vision caption + pinned description. Everything else is skipped or already present. |

## 8. Authorised evaluation procedure for #77 (proposal; **nothing authorised yet**)

Owner prerequisites and approvals: (1) approve spending up to a hard cap (proposal: $3.50 of the $5); (2) approve each upload (the
owner wants to see what is sent every time); (3) use only drawings the owner made, no private media; (4) a fal key stored outside the repo
(env `FAL_KEY`), never committed; (5) read https://fal.ai/privacy and the CDN public-URL note, and decide whether to set
`X-Fal-Object-Lifecycle-Preference` / use `sync_mode`.

Test set: 1 rough bird, the same bird plus a drawn branch (and a mask of the branch area), 2 wing-up/down rough poses, and 4 walking-dog
poses with one approved reference.

Calls (at roughly 1 MP, prices above; costs are **list-price estimates**, billed per rounded-up megapixel):

| Step | Calls | Estimated cost |
|---|---|---|
| Bird to image: R3 x3, R1 (canny) x3, R4 x3 | 9 | 3 x 0.04 + 3 x 0.075 + 3 x 0.0398 = about $0.46 |
| Branch addition with mask: R2 x3, R1 (mask) x3, R3 (no mask, shows drift) x3 | 9 | 3 x 0.05 + 3 x 0.075 + 3 x 0.04 = about $0.50 |
| Same dog, 4 poses, 2 tries: R3, R4 and R1 (reference) | 24 | about $1.2 |
| **Total** | **42** | **about $2.2; cap $3.50** |

Time: about 1 to 2 hours of the owner's time (an estimate; per-call latency is not documented). Method: drive them through fal's
playground in the browser or a small script **only after the owner approves**; record endpoint, version, parameters, request id, latency,
cost and the output; judge interpretation, preservation of approved pixels (diff the unmasked area), palette and line drift, and identity
across poses; stop at the cap. Result: proceed / revise / stop per #77. A fixed seed is not proof.

## 9. Not verified

- No behaviour of any model was tested. Every "follows a sketch", "identical character" and "approved pixels preserved" claim is UNTESTED.
- fal: `flux-general` combined inputs, inpaint behaviour, data-URI acceptance per endpoint, latency, max size, alpha, whether fal trains on inputs, ideogram character price. The Kontext price differs between the two fal pages read.
- Other fal endpoints (for example newer FLUX, Qwen-Image-Edit, Seedream, Recraft) were not read.
- Local: no M4 latency or memory for any model; SDXL inpainting, ControlNet SDXL scribble/lineart cards; sd.cpp inpaint/server docs; Draw Things HTTP API; ComfyUI `/prompt` payload; Ollama Metal statement and cancel; vision-model licences; licence text of SDXL OpenRAIL++-M beyond the card.
- WaltCut's existing stabiliser, pressure and lasso code was taken from the owner's statement, not re-verified in the source.
