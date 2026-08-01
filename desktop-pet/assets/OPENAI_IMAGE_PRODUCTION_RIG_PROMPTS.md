# OpenAI Image prompt pack: production motion rig

Generate these files **sequentially**, not in one request. For each generation,
attach the identity reference named in the step. After a step is approved, use
its output as the primary reference for the next step. This is more reliable
than asking the model for a sprite sheet.

The prompts are written specifically for OpenAI Image. Put approved outputs in
`desktop-pet/assets/source/` using the exact file names below.

## Step 1 — clean front identity master

Reference image: `source/stand.png`

Output: `source/rig_master_front.png`

```text
Edit the supplied character reference into a clean production animation-rig
source. Preserve the exact same original character identity, silhouette,
proportions, large pointed cybernetic ears, hood geometry, glossy oval visor,
oversized technical jacket, trousers, rounded paws, luminous tail, line weight,
cel-shaded rendering and material detail. This is an original small
hacker-alien creature with playful mischievous energy. Do not imitate or copy
any existing franchise character.

Keep a strict orthographic front view and the same standing height and body-to-
head ratio. Place the feet on one shared baseline. Move the arms only enough to
make them riggable: both arms are fully visible beside the torso, slightly away
from the jacket, elbows relaxed, paws visible, with clean background gaps from
upper sleeve to wrist. Separate the legs slightly so both complete legs and
feet have clean independent silhouettes. Keep the torso and jacket complete
behind the limbs.

Remove the floating orb from this image. Remove every free-floating square,
pixel block, glitch tile and particle around the ears, body and tail. Keep only
small embedded technical light panels that are physically part of the clothing.
The visor must be blank, dark and glossy: remove the eyes, mouth, symbols and
all baked facial graphics.

Use the same premium deep-violet and near-black material palette, restrained
magenta construction accents and electric-cyan technical lights. Do not make
every accent equally bright; the character must read clearly without a noisy
halo.

Create exactly one full-body character on a 1024 x 1536 transparent canvas.
No floor, no cast shadow, no scenery, no text, no logo, no watermark, no extra
objects and no extra characters. If real alpha transparency cannot be produced,
use a perfectly flat #202126 background with no gradient, texture or bloom.

Do not redesign the ears, hood, visor, jacket, tail, paws, colour placement or
body proportions. Do not crop any body part. Do not add fur realism, weapons,
backpack, cables, wings or decorative particles.
```

## Step 2 — clean right-profile rig master

References, in priority order:

1. approved `source/rig_master_front.png` — character identity;
2. `source/profile_arms_right.png` — strict side-view pose only.

Output: `source/rig_master_profile_right.png`

```text
Using the approved front rig master as the primary identity reference, create
the exact same character in a strict orthographic 90-degree side profile facing
right. Use the secondary side image only to understand the viewing angle; do
not copy its background or any identity drift.

Preserve the approved character's exact proportions, silhouette language,
large cybernetic ears, hood, blank glossy visor, oversized jacket, trousers,
rounded paws, luminous tail, line weight, cel shading, deep-violet and near-
black materials, restrained magenta accents and electric-cyan technical lights.

This is a source for a segmented premium 2D motion rig. Show both complete arms:
the far arm behind the torso and slightly forward, the near arm in front and
slightly backward. Give both elbows a gentle natural bend. Leave an unbroken
background gap around every forearm and paw. Show both complete legs with the
far leg slightly backward and the near leg slightly forward. Leave background
gaps between thighs, lower legs and feet. Keep shoulder, elbow, hip, knee and
ankle locations anatomically clear. Reconstruct a complete torso beneath the
limbs. The tail must be fully visible and separated from jacket and legs.

No orb. No free-floating square particles, pixel blocks, glitch tiles or halo
around the creature. Embedded clothing lights are allowed. The visor contains
no face or symbols.

Exactly one full-body character, 1024 x 1536 transparent canvas, same character
height and foot baseline as the front master. No floor, shadow, scenery, text,
logo, watermark or extra object. Do not create a three-quarter view, sprite
sheet, T-pose, action pose or multiple variations.
```

## Step 3 — ledge-grab key pose

Primary reference: approved `source/rig_master_profile_right.png`

Output: `source/pose_ledge_grab_profile_right.png`

```text
Edit the supplied approved right-profile rig master into one clean ledge-grab
key pose. Preserve the exact character, strict right-facing profile, scale,
palette, materials, outline and rendering style. Change only the pose.

The character is hanging from an invisible horizontal ledge above its head.
Both complete arms reach upward, both elbows slightly bent under load, and both
paws grip the same horizontal contact level. The shoulders are raised naturally
without changing the jacket design. The torso hangs below the hands with a
subtle stretch. Both legs trail downward with soft knee bends and clear negative
space between them. Feet are relaxed. The tail hangs lower and lags behind the
body. The pose must show believable weight, not a floating standing pose.

Do not draw the ledge or any scenery; the real desktop window edge supplies the
contact surface at runtime. Keep all arms, paws, legs and feet complete and
separable. Blank glossy visor. No orb. No free-floating square particles around
the creature. Exactly one character on a 1024 x 1536 transparent canvas. No
floor, shadow, text, logo, watermark or additional objects.
```

## Step 4 — Orb Lift hanging key pose

Primary reference: approved `source/rig_master_profile_right.png`

Secondary reference: current `orb.png` for the orb identity.

Output: `source/pose_orb_lift_profile_right.png`

```text
Edit the supplied approved right-profile character rig into one Orb Lift key
pose. Preserve the exact same original character, strict right-facing profile,
scale, palette, materials, outline and rendering style. Preserve the supplied
orb design exactly.

Place the orb above and slightly in front of the character's head. The character
hangs beneath it and grips the lower sides of the orb securely with both paws.
Both complete arms reach upward with slightly bent elbows and visibly carry the
body weight. The torso stretches subtly below the shoulders. Both knees bend
backward from upward acceleration, the feet trail below, and the tail streams
downward with delayed inertia. The pose communicates that the orb is actively
lifting the character, not that the character can fly.

Only the orb may have a small cluster of free-floating square/pixel particles.
There must be no square particles around the creature, ears, jacket, legs or
tail. Keep embedded clothing lights. The visor remains blank and glossy.

Exactly one character and one orb on a 1024 x 1536 transparent canvas. Keep all
limbs complete and visually separable. No floor, ledge, scenery, cast shadow,
text, logo, watermark, motion blur or extra character.
```

## Step 5 — pull-up compression key pose

Primary reference: approved `source/rig_master_profile_right.png`

Output: `source/pose_pull_up_profile_right.png`

```text
Edit the approved strict right-profile character into the compression phase of
a ledge pull-up. Preserve the exact character identity, profile direction,
scale, palette, material detail, line weight and cel shading.

Both paws remain fixed at one invisible horizontal contact line near chest
height. The elbows bend deeply as the character pulls the chest above that
line. The shoulders and torso compress naturally, one knee rises toward the
ledge, the other leg trails below, and the tail counterbalances behind. The
silhouette must clearly communicate effort, weight transfer and preparation to
plant a foot. Keep both arms, paws, legs and feet complete and separable.

Do not draw the ledge. Blank glossy visor. No orb. No free-floating square
particles around the creature. Exactly one full character on a 1024 x 1536
transparent canvas. No floor, shadow, scenery, text, logo or watermark.
```

## Generation order and stop rule

Generate Step 1 first and inspect it before continuing. Do not compensate for a
bad Step 1 in later prompts. Reject and regenerate an image if identity changes,
if free particles remain around the creature, if the visor contains a face, or
if any required limb is fused/cropped. Once Step 1 is approved, use it as the
identity source for all later generations.
