name: Feature Request
about: Suggest an idea for this project
title: "feat(): "
labels: ["enhancement", "triage"]
body:
  - type: markdown
    attributes:
      value: |
        Thanks for taking the time to suggest a new feature!
  - type: textarea
    id: problem
    attributes:
      label: Problem
      description: What problem are you trying to solve?
      placeholder: Describe the problem or limitation
    validations:
      required: true
  - type: textarea
    id: solution
    attributes:
      label: Proposed Solution
      description: Describe the solution you'd like
      placeholder: Describe the proposed feature
    validations:
      required: true
  - type: textarea
    id: alternatives
    attributes:
      label: Alternatives Considered
      description: What alternatives have you considered?
      placeholder: Describe alternative approaches
    validations:
      required: false
  - type: textarea
    id: context
    attributes:
      label: Additional Context
      description: Any other context or screenshots
      placeholder: Add any other context
    validations:
      required: false
