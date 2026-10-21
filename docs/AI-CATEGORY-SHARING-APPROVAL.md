# Proposed AI category sharing
Date: 2026-10-11
Status: owner authorized an opt-in setting; local is the default (2026-10-11).

## Concrete proposed behavior
Only after a user selects models and clicks Generate suggestions:
- send the normalized preview and display name (already implemented);
- send existing tag names with each assigned tag's category;
- when existing-only categories are selected, send the bounded list of existing
  category names so the provider can use the correct category vocabulary.
Do not send model paths, original file bytes, notes, URLs, designer, print history,
filaments, credentials in the body, or other models' tag names.
No provider calls occur when opening/saving settings or while scanning.

Synthetic example (no user catalog or credentials):
```json
{
  "displayName": "Calibration cube",
  "existingTags": [{"name": "calibration", "category": "Workshop"}],
  "existingCategories": ["Workshop", "Decor"]
}
```
The preview is separately attached as a bounded PNG. API credentials remain
session-only and go only in the configured provider's authentication header.
The selected provider may be a local/private compatible endpoint or remote HTTPS.

## Approval boundary
Automatic approval review rejected patches adding categories/taxonomy to the
external payload as internal metadata and required explicit human authorization.
No rejected payload is implemented or transmitted. Current local review DTOs
contain categories for accurate before/after displays; categories are locally
validated at generation/review apply. Current production provider payload contains
names only. Approval would authorize this product feature after an explicit
Generate action, not unattended calls, real test calls, spending or publication.

## Verification before enabling
Extend strict provider input bounds and mock request fixtures to check the exact
fields and absence of paths/notes/history/secrets. Update the visible disclosure
to match the approved payload. Run native/frontend gates and both app builds.

## Owner decision
After the explanation of exact fields, configured provider and explicit Generate
action, the user instructed: "make this a setting the user can choose. Local is default".
Implement categorySharing=local|provider, default local in new/older settings.
Local sends names only; provider opt-in sends assigned name/category pairs and
bounded relevant category vocabulary after explicit generation. Changing the flag
invalidates in-flight reviews. Keep it distinct from the AI endpoint scope so
"local categories" cannot imply that the selected AI itself runs locally.
This resolves the prior source-egress gate for this conditional product feature.
No real provider calls, account actions, charges or unattended sharing are authorized.
