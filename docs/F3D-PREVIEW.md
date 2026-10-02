# Autodesk Fusion previews

F3D files remain original local files. GoggleLab asks the installed Fusion bridge to export
a temporary STEP preview, then displays its tessellation through the STEP worker.
No design is uploaded. Fusion must be installed and running with the bridge enabled.

The Connect Fusion action installs the add-in from `integrations/fusion/GoggleLabFusionBridge`.
Enable GoggleLabFusionBridge and Run on Startup in Fusion's add-in manager.
The bridge uses a private local spool with request IDs and bounded source/output sizes.
Cancellation invalidates old requests and completed jobs are cleaned up.

Run bridge tests with `python3 integrations/fusion/test_bridge.py`.
The original design's size and digest remain its identity; temporary STEP metadata does not replace them.
F3D is a viewing workflow and does not promise CAD editing or slicer handoff.
