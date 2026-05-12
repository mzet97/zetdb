name: Bug Report
about: Create a report to help us improve
title: "fix(): "
labels: ["bug", "triage"]
body:
  - type: markdown
    attributes:
      value: |
        Thanks for taking the time to fill out this bug report!
  - type: textarea
    id: expected
    attributes:
      label: Expected Behavior
      description: What did you expect to happen?
      placeholder: Describe the expected behavior
    validations:
      required: true
  - type: textarea
    id: actual
    attributes:
      label: Actual Behavior
      description: What actually happened?
      placeholder: Describe the actual behavior
    validations:
      required: true
  - type: textarea
    id: reproduce
    attributes:
      label: Steps to Reproduce
      description: Provide a minimal reproducible example
      placeholder: |
        1. Start server with `cargo run --release`
        2. Run command `SET key value`
        3. Observe error
    validations:
      required: true
  - type: textarea
    id: context
    attributes:
      label: Context
      description: Version, OS, runtime details
      value: |
        - ZetDB version:
        - Rust version:
        - OS:
        - Architecture:
    validations:
      required: true
  - type: textarea
    id: logs
    attributes:
      label: Logs
      description: Relevant log output
      render: shell
    validations:
      required: false
