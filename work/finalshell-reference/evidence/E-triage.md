# E-triage: FinalShell reference JAR

## Sample

- Path: `E:\Projects\FirstShell\finalshell.jar`
- Size: 41,090,830 bytes
- SHA-256: `7440BB1EAF590E5FE695ECD7E290BC85BEFCCA5B69478E6E1775D466357C6ACC`
- Format: Java JAR
- Manifest main class: `st`
- Manifest runtime: Oracle JDK 11.0.1
- Network profile: offline; the sample was not executed

## Inventory

- 13,297 class entries, including 4,318 root-level obfuscated classes.
- 124 JSON terminal themes, 22 bundled TTF fonts, and 608 bitmap assets.
- Notable bundled libraries: Pcap4J, OSHI, RSyntaxTextArea, Sockslib, Apache libraries, Guava, JNA, and XStream.
- The manifest does not identify the FinalShell product version. `resources/version.xml` belongs to WebLaF and is not a product-version record.

## Static anchors

- Proxy settings and host/port UI: `artifacts/jadx/sources/p000/ahC.java:922` and `:929`.
- System proxy selection and persistence: `artifacts/jadx/sources/p000/C5371pW.java:936` through `:964`.
- Editable history state: `artifacts/jadx/sources/p000/agX.java:47` through `:48`, with save/restore methods at `:356` and `:382`.
- Path autocomplete state and popup: `artifacts/jadx/sources/p000/ajW.java:34` through `:38`, and `RunnableC2760gE.java:25` through `:29`.
- Styled editor component: `artifacts/jadx/sources/p000/aiF.java:902`.
- Proxy implementation library: 137 `sockslib` classes.
- System/network inspection libraries: 252 OSHI classes and 596 Pcap4J classes.

## Quality and limits

- JADX processed 20,980 inputs and emitted 9,353 Java files, but completed with 5,437 errors.
- The business classes use identifier and string obfuscation. Decompiled control flow is not reliable enough to reproduce implementation details.
- Library presence establishes available building blocks, not that every library feature is reachable in the product UI.
- Feature scope must therefore be based on observable UI behavior plus official feature documentation, with this static inventory used only as corroboration.

## Decision

Use a clean-room compatibility process: record screens, settings, input/output behavior, and failure states; implement independently with original source, icons, themes, and product naming. Do not copy decompiled methods or bundled proprietary assets.
