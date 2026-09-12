---
id: TASK-145
title: A source with more than one audio stream exports only its first
status: To Do
assignee: []
created_date: '2026-09-12 04:33'
labels:
  - audio
  - export
  - bug
dependencies: []
ordinal: 165000
---

## Description

<!-- SECTION:DESCRIPTION:BEGIN -->
The user's 4K60 test footage carries three 48 kHz stereo AAC tracks, and the export matrix (TASK-143) shows every export of it - every encoder, every preset, both the CLI and the GUI - writing one stereo track. uridecodebin exposes one audio pad per stream only when it is asked to; sub_media::decode opens a single audio branch, so the second and third tracks are dropped without a warning anywhere: not in the media bin, not in the render report, not in the export warnings. A multi-track camera master or a mix-minus feed is a normal thing to cut, and silently keeping only track one is the kind of loss a user finds after delivery.
<!-- SECTION:DESCRIPTION:END -->

## Acceptance Criteria
<!-- AC:BEGIN -->
- [ ] #1 The media probe reports every audio stream a file carries, and the bin and the inspector show how many there are
- [ ] #2 A clip names which of the source's audio streams it takes, defaulting to the first, and the decoder opens that one
- [ ] #3 An export whose source carries streams the project does not use says so in its warnings rather than dropping them silently
<!-- AC:END -->
