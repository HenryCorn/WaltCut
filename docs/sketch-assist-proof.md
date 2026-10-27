# Sketch Assist proof: run sheet for issue #77

**Nothing in this document, and nothing in `tools/sketch-assist-proof/`, has been run against fal.ai.** No key was
available to the author, no request was sent, nothing was uploaded or spent. The tool was tested only with a dry run and
a local fake server. Every request shape, price and behaviour below is from fal's public docs as read on 2026-10-27, and
the real service may differ. The first live call is the first real test of the tool itself, so keep an eye on the
spend line (`python3 run.py spent`) and on your fal dashboard.

You make the decision (proceed / revise / stop) in [#77](https://github.com/HenryCorn/WaltCut/issues/77). The kit only
makes it cheap, visible and safe to gather the evidence. Background: [`sketch-assist-backends.md`](sketch-assist-backends.md).

## What the kit does and refuses

- Reads your key **only** from the environment variable `FAL_KEY`. There is no key option on the command line, and the key
  is removed from anything printed or saved.
- Default is a **dry run** that sends nothing. Real calls need `--live`, and **every call asks you to type `y`**, after
  showing you the exact files (path, pixel size, bytes, hash) and parameters it will upload. There is no "yes to all".
- A **hard spend cap**: default 3.50 USD, `--cap` can lower it, anything above 5.00 is refused. Before each call it adds the
  estimate to the ledger total and refuses if that would pass the cap. Cancelled and failed calls still count (they may be
  billed). The ledger is `~/waltcut-proof/ledger.jsonl`, written before the call and updated after.
- Reads only files you name on the command line. Refuses folders, anything over 4096 px or 20 MB, and anything that is not
  PNG or JPEG. Refuses to overwrite existing outputs (use a new `--run` name).
- Images are sent **inline** (base64 in the request) rather than uploaded to a URL. It also asks fal for short retention
  (`X-Fal-Object-Lifecycle-Preference` of one hour), no request-history storage (`X-Fal-Store-IO: 0`) and `sync_mode`.
  Sources: the [FAQ](https://fal.ai/docs/model-apis/faq) (media on the CDN "at least 7 days by default", public to anyone
  with the link) and [media expiration](https://fal.ai/docs/model-apis/media-expiration). **Not verified:** whether fal
  stores inline inputs or trains on them; read <https://fal.ai/privacy> before the first upload.
- Ctrl-C while waiting sends fal's documented queue cancel (`PUT .../requests/{id}/cancel`) and records it. A timeout
  (default 300 s per call) does the same.

All commands are run from the repo's `tools/sketch-assist-proof/` folder: `cd tools/sketch-assist-proof`.
Self-test (no network, no key): `python3 -m unittest`.

## Costs (list prices checked 2026-10-26; fal changes them)

Billing is per megapixel, rounded up, so **keep images at or under 1 megapixel** (for example 1024x768 or 1280x720).
A 1920x1080 export is 2.07 MP and costs 3 times as much per call.

| Recipe | Estimate at up to 1 MP | Note |
|---|---|---|
| `kontext` (`fal-ai/flux-pro/kontext`) | 0.055 USD | fal's two pages disagree (0.04 per image vs 0.055 per MP); the kit assumes the higher |
| `nano-banana` (`fal-ai/nano-banana/edit`) | 0.0398 USD | cheap control |
| `flux-fill` (`fal-ai/flux-pro/v1/fill`) | 0.05 USD | needs a mask |
| `flux-general` (`fal-ai/flux-general`) | 0.075 USD | **EXPERIMENTAL, off by default** |

Planned spend for the whole proof, 3 tries per step: about 0.3 USD per step, roughly 1 USD in total, far below the cap.

## Step 0. Draw (about 20 minutes)

Use only drawings you made. A stick-figure dog is fine. You need four pictures:

1. **Rough dog**: a side-on walking dog, loose lines, in one or two colours.
2. **Added stroke**: after step 2 below, the same approved picture plus one new mark, for example a collar or a tail.
3. **Pose A**: an approved picture of the dog (the result of step 1 that you liked).
4. **Pose B**: a new rough sketch of the same dog with different legs (mid-step).

To draw in WaltCut ([`animation.md`](animation.md)): put the playhead on a frame, choose **Graphics and Titles ▸ New Layer ▸
New Drawing**, pick the **Brush** tool (`Shift+P`), and draw in the Program monitor. For a new pose, move the playhead and
choose **New Cel**.

### Exporting a PNG from WaltCut

The Program monitor has a **camera button**. It opens **Export Frame**, where you choose a name, format (PNG), the
folder (Browse) and click OK ([`monitors.md`](monitors.md), "Export Frame"). The command behind it is `file.exportFrame`.
It exports the Program picture at the playhead, so move the playhead to the cel you want first.

Things I could not check, so look at each PNG before sending it:

- Is the sequence small enough (about 1 MP)? If not, the cost estimate in the plan will show it. Smaller sequence
  settings make cheaper calls.
- Is the background transparent (checkerboard in Preview) or black? The models were not tested with transparency. Safer: import a
  plain white picture (`python3 run.py make-blank --color white --size 1280x720 --out ~/white.png`, then import
  it as footage) and put it on the track under your drawing before exporting.
- Put the exports in one folder, for example `~/proof-in/`, named `dog-rough.png`, `dog-approved.png`,
  `dog-approved-plus-stroke.png`, `dog-posea.png`, `dog-poseb-rough.png`.
- **Mask (optional, only for the `flux-fill` recipe):** a black picture the same size as the approved one with the new
  area painted white. I did not find a WaltCut-only way to make that, so use Preview or any paint program. Whether fill
  regenerates white or black is not stated on fal's page: the report will show you. Skip the mask by using `--only kontext,nano-banana`.

## Step 1. Rough dog -> Enhance (scenario `single-enhance`)

```
python3 run.py plan single-enhance --sketch ~/proof-in/dog-rough.png
python3 run.py run  single-enhance --sketch ~/proof-in/dog-rough.png --run s1-try1            # dry run, shows what would be sent
export FAL_KEY=...   # in this terminal only; never paste it into a file, chat or issue
python3 run.py run  single-enhance --sketch ~/proof-in/dog-rough.png --run s1-try1 --live
```

Estimated cost: about 0.095 USD (kontext 0.055 + nano-banana 0.0398). Read each confirmation screen; type `y` to send, anything
else skips that call. Change the wording with `--prompt "..."`; the default asks for a walking dog in
"storybook, flat cartoon, ink line, cut-out paper". Try 2 or 3 times (`--run s1-try2`, ...) with `--prompt` variants; a new `--run` name is required each time.

Look for: Did it keep **my lines loosely** (same dog, same direction, same legs, nothing extra)? Is the style the one you
wanted? Did it add a background, extra animals or text?

## Step 2. Add a stroke without redesign (scenario `additive-edit`)

Pick the result you approved and save it as `dog-approved.png` (same pixel size as the others). Draw one new mark (collar
or tail) on top of it in WaltCut, export that as `dog-approved-plus-stroke.png`.

```
python3 run.py run additive-edit --approved ~/proof-in/dog-approved.png --sketch ~/proof-in/dog-approved-plus-stroke.png --prompt "a red collar" --only kontext,nano-banana --run s2-try1
python3 run.py run additive-edit --approved ~/proof-in/dog-approved.png --mask ~/proof-in/collar-mask.png --prompt "a red collar" --only flux-fill --run s2-fill1
```
(add `--live` after the dry run, as above.) Estimated cost: kontext 0.055 + nano-banana 0.0398 + fill 0.05, about 0.145 USD per try.

Look for: did the collar appear, **and did the rest of the dog stay the same** (lines, colours, legs, background)? Flip between
`dog-approved.png` and the output. Note anything that moved, even slightly. Fill should leave the unmasked area alone; kontext and nano-banana have no mask, so they show how much drift happens without one.

## Step 3. Two poses of the same dog (scenario `two-poses`)

```
python3 run.py run two-poses --reference ~/proof-in/dog-posea.png --sketch ~/proof-in/dog-poseb-rough.png --prompt "walking, left front leg forward" --run s3-try1
```
(add `--live` after the dry run.) Estimated cost about 0.095 USD. Note that `kontext` accepts one image: it gets pose A and a
text description only, so the sketch is **not** sent to it; `nano-banana` gets both images. Do 2 tries per
pose pair, and a second pose pair if budget allows.

Look for: same dog (shape, markings, colours, line style) in both poses? Right legs and direction? Check the pose of B follows your sketch.

## Experimental recipe (skip unless asked)

`flux-general` (sketch control + reference + mask in one call) is **EXPERIMENTAL**: no official example combines those fields
and the request is built from the schema alone. It is off by default. It also needs Hugging Face repo paths for the
ControlNet (`--controlnet-union-path`, and for the IP-Adapter `--ip-adapter-path` and `--ip-adapter-encoder-path`) that fal's
page does not give defaults for, so the kit does not invent any. Enable it only with `--include-experimental`. Record a failure
here as a failure, not as "not tried".

## Step 4. Contact sheet and totals

```
python3 run.py report --run s1-try1        # writes ~/waltcut-proof/runs/s1-try1/contact-sheet.html (local file, open in a browser)
python3 run.py spent
```
Compare `spent` with your fal dashboard and write both numbers down: the kit's figure is a list-price estimate.
Outputs, `*.request.json` (no key) and `*.call.json` are in `~/waltcut-proof/runs/<run>/`. The ledger is `~/waltcut-proof/ledger.jsonl`.

## Scoring sheet

Fill one row for **every** call, including bad ones and ones that failed or were refused by the safety checker. Do not
only keep the best.

| Run / call | Step | Recipe | Kept my lines loosely (0-2) | Rest unchanged / same dog (0-2) | New mark appeared (0-2) | Style OK (0-2) | Time (s) | Est. cost | Notes (what drifted, what broke) |
|---|---|---|---|---|---|---|---|---|---|
| | | | | | | | | | |

0 = failed, 1 = partly, 2 = good. Add the failed attempts, errors and cancelled calls as rows too.

Decision (your call; the ladder is the same as #77):

- **Proceed**: at least one recipe scores 2 on "rest unchanged" in most step 2 tries **and** the two poses look like the same dog in most step 3 tries, at a cost and time you accept (60 s per Enhance is your stated limit).
- **Revise**: some steps work and others do not; say which recipe, which step and what to change (mask, a different reference, another endpoint).
- **Stop**: the dog changes when it should not, or the poses are not one dog, on most tries.
  "Identical" is a goal no backend promises, and a fixed seed is not evidence.

## What to paste back into #77

Do not paste the key, and do not paste your drawings unless you want to. Paste:

```
Proof run date:
Steps run: single-enhance x_, additive-edit x_, two-poses x_
Recipes tried (and any EXPERIMENTAL):
Kit total (python3 run.py spent):            fal dashboard total:
Per recipe: calls, median latency (s), failures/refusals:
Step 1 (kept lines loosely):                 
Step 2 (rest unchanged, mark appeared):      
Step 3 (same dog across poses):              
Anything that surprised you (errors, wrong shapes, mask polarity, transparent background, price differences):
Scoring table: (paste)
Decision: proceed / revise / stop     Because:
```
Attach the `contact-sheet.html` or screenshots of it only if you are happy to share them.
