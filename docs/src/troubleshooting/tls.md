# TLS Certificate Errors

If you're receiving certificate errors such as this one:

```
invalid peer certificate: UnknownIssuer
```

This is probably because the TLS certificate of the server you're hitting is expired, invalid, or self-signed. The best solution is to fix the error on the server, either by renewing the certificate or creating a signed one. In most cases this is the best solution. If not possible, you should just disable TLS on your server because it's not doing anything for you anyway.

If you can't or don't want to fix the certificate, and you need to keep TLS enabled for some reason, it's possible to configure Slumber to ignore TLS certificate errors on certain hosts.

> **WARNING:** This is dangerous. You will be susceptible to MITM attacks on these hosts. Only do this if you control the server you're hitting, and are confident your network is not compromised.

- Open your [Slumber configuration](../api/configuration.md)
- Add the field `ignore_certificate_hosts: ["<hostname>"]`
  - `<hostname>` is the domain or IP of the server you're requesting from
