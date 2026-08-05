This PR template helps ensure that as we launch new features we appropriately communicate, track, document, make them accessible, etc.

## PRD checklist

- [ ] Plan for how to measure success quantified by metrics

## Coding checklist

- [ ] Test in dev for a week
- [ ] Telemetry in code
- [ ] A11y (if applicable)
- [ ] Add to Command Palette (if applicable)
- [ ] Add toggle setting(s) to command palette (if applicable)
- [ ] Add to Mac Menu (if applicable)
- [ ] Add keybinding (if applicable), see [actions audit for inspiration](https://docs.google.com/spreadsheets/d/1C56ZIqDGjJi873-HAPdnT2DofC3Z6G-aJMYeQeERHx4/edit#gid=0)
- [ ] Sanity check within the app that it does not clash other keybindings
- [ ] No sensitive info in logs
- [ ] No crashes on dev related to the feature
- [ ] No known performance regression on dev
- [ ] Feature works fine, and no regression, over SSH. See [the local SSH test instructions](../../app/tests/ssh/README.md).
- [ ] Have we explicitly brainstormed how this feature will be discovered by developers?
- [ ] Link to Figma mocks
- [ ] Tested on multiple themes (both dark and light)
- [ ] If the feature relies on an external service/API, its compatibility and rollback plan are documented


## Content checklist

- [ ] Help content
- [ ] Changelog entry (add entry below)
- [ ] Telemetry entry (if applicable)
- [ ] Metrics or observability plan (if applicable)
- [ ] Tweet (if appropriate)
- [ ] Blog post (if appropriate)

## Changelog

CHANGELOG-NEW-FEATURE: {{Insert a changelog entry here}}
