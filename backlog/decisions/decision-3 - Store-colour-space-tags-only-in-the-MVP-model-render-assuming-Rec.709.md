---
id: decision-3
title: Store colour space tags only in the MVP model; render assuming Rec.709
date: '2026-09-08 20:52'
status: accepted
---
## Context

A full colour management pipeline is out of MVP scope, but throwing away colour metadata now would make it lossy to add later.

## Decision

The model stores colour space, transfer and primaries tags on media items and sequences. Rendering assumes Rec.709 in the MVP.

## Consequences

Colour transforms become a post-MVP, plugin-extensible concern. No data migration is needed when they arrive.
