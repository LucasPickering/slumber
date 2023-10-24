# Quick Start

## 1. Installation

See [install instructions](/artifacts/)

## 2. Create a Slumber collection file

Create a file called `slumber.yml` and add the following contents:

```yaml
requests:
  - id: get
    method: GET
    url: https://httpbin.org/get
```

## 3. Run Slumber

```sh
slumber
```
