<!DOCTYPE html>
<html lang="en">
  <head>
    <meta charset="utf-8">
    <title>Redirecting...</title>
    <meta http-equiv="refresh" content="0; URL=/user_guide/tui/filter_query.md">
    <link rel="canonical" href="/user_guide/tui/filter_query.md">
  </head>
  <body>
      <p>Redirecting to... <a href="/user_guide/tui/filter_query.md">/user_guide/tui/filter_query.md</a>.</p>

<script>
    // This handles redirects that involve fragments.
    document.addEventListener('DOMContentLoaded', function() {
        const fragmentMap =
            {}
        ;
        const fragment = window.location.hash;
        if (fragment) {
            let redirectUrl = "/user_guide/tui/filter_query.md";
            const target = fragmentMap[fragment];
            if (target) {
                let url = new URL(target, window.location.href);
                redirectUrl = url.href;
            } else {
                let url = new URL(redirectUrl, window.location.href);
                url.hash = window.location.hash;
                redirectUrl = url.href;
            }
            window.location.replace(redirectUrl);
        }
        // else redirect handled by http-equiv
    });
</script>
  </body>
</html>
