# Archived prompt: rejected side-profile arm candidate

`source/profile_arms_right.png` is retained for provenance but is not a runtime
input. Its shallow three-quarter shoulders do not match the approved strict
profile torso. `extract_profile_motion.py` deliberately derives both arm depth
layers from the one complete strict-profile sleeve in
`source/rig_master_profile_right.png`, preventing the attachment mismatch from
returning through asset regeneration.

The prompt below is historical and must not be used to replace the active rig.

Use the newly generated right-facing side-profile image as the **primary
reference**. Use `source/stand.png` only as a secondary identity reference.
Generate one image and save it as `source/profile_arms_right.png`.

Paste the following prompt into the image generator:

```text
Edit the supplied right-facing side-profile character image. Preserve the
existing image exactly wherever no change is explicitly requested. Keep the
same character, strict right-facing side profile, canvas, scale, position,
ground baseline, proportions, head, hood, ears, visor, jacket, trousers, legs,
feet, tail, colours, lighting, outlines and rendering style.

Change ONLY the arms and the jacket areas previously occupied by the hands in
the pockets.

The character is being prepared as a layered animation rig for natural walking
and running. Remove both hands from the jacket pockets and show two complete,
separate arms in a relaxed counter-swing pose:

- The NEAR arm is fully visible and swings diagonally backward from the near
  shoulder. Its elbow is gently bent approximately 25 degrees. The complete
  sleeve, wrist and small rounded paw are visible.
- The FAR arm is visible behind the torso and swings diagonally forward from
  the far shoulder. Its elbow is gently bent approximately 25 degrees. The
  complete sleeve, wrist and small rounded paw are visible.
- Both arms must preserve the character's oversized magenta-violet jacket
  sleeves, cyan illuminated details, cuff construction, bold outline, cel
  shading and glossy highlights.
- The arms must clearly belong to different depth layers: far arm behind the
  torso, near arm in front of the torso.
- Except for the small natural shoulder connection, each arm must have a clear
  silhouette. Leave visible background space between the arms and the torso
  from the upper sleeve downward. Do not merge either forearm or paw into the
  jacket body, trousers, tail or the other arm.
- Both shoulder pivot locations must be anatomically clear and aligned, so the
  arms can later be cut out and rotated independently in code.
- Reconstruct the jacket body and both pockets naturally beneath the removed
  arms. The torso must look complete if the arms are digitally removed.
- Keep both empty pockets visible. No hands remain inside them.

Do not change the pose of the head, torso, tail, legs or feet. This is not a new
walking frame and not a running frame; it is a neutral rigging source image
containing clean, extractable arm layers.

Maintain the strict 90-degree right-facing profile. Do not rotate the chest,
visor or feet toward the viewer. The visor remains blank and contains no face.

Create exactly ONE full-body character on the same vertical canvas as the
supplied side-profile image. Use the same flat dark background. No floor, no
cast shadow, no particles, no floating orb, no text, no logo, no watermark and
no additional objects.

DO NOT PRODUCE: front view, three-quarter view, symmetrical frontal shoulders,
hands in pockets, missing far arm, hidden paw, fused arms, arms crossed over the
torso, arms touching the legs, straight horizontal T-pose, raised arms, waving,
combat pose, sprinting pose, extra limbs, changed jacket, changed body shape,
changed tail, changed legs, different character, sprite sheet or multiple
variations.

FINAL SELF-CHECK:
1. Is the character still the same strict right-facing profile?
2. Are two complete arms visible, one forward and one backward?
3. Can each arm be cut out as an independent layer without cutting into the
   torso, trousers or tail?
4. Is the jacket body complete underneath the arms?
5. Are both hands completely outside the pockets?

If any answer is no, correct the geometry before producing the final image.
Arm separability is more important than making the pose dramatic.
```

The output must be a single PNG. Do not ask the generator for multiple frames:
the animation will be built from extracted layers in Rust so that identity and
timing remain stable.
