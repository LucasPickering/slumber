# Quick Start

Once you've [installed Slumber](/artifacts), setup is easy.

## 1. Create a Slumber collection file

Create a file called `slumber.yml` and add the following contents:

```yaml
requests:
  get:
    method: GET
    url: https://httpbin.org/get
```

## 2. Run Slumber

```sh
slumber
```
