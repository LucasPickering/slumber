// Populate the sidebar
//
// This is a script, and not included directly in the page, to control the total size of the book.
// The TOC contains an entry for each page, so if each page includes a copy of the TOC,
// the total size of the page becomes O(n**2).
class MDBookSidebarScrollbox extends HTMLElement {
    constructor() {
        super();
    }
    connectedCallback() {
        this.innerHTML = '<ol class="chapter"><li class="chapter-item expanded "><a href="introduction.html"><strong aria-hidden="true">1.</strong> Introduction</a></li><li class="chapter-item expanded "><a href="install.html"><strong aria-hidden="true">2.</strong> Install</a></li><li class="chapter-item expanded "><a href="getting_started.html"><strong aria-hidden="true">3.</strong> Getting Started</a></li><li class="chapter-item expanded affix "><li class="part-title">User Guide</li><li class="chapter-item expanded "><a href="user_guide/key_concepts.html"><strong aria-hidden="true">4.</strong> Key Concepts</a></li><li><ol class="section"><li class="chapter-item expanded "><a href="user_guide/recipes/index.html"><strong aria-hidden="true">4.1.</strong> Recipes</a></li><li><ol class="section"><li class="chapter-item expanded "><a href="user_guide/recipes/bodies.html"><strong aria-hidden="true">4.1.1.</strong> Bodies</a></li></ol></li><li class="chapter-item expanded "><a href="user_guide/templates/index.html"><strong aria-hidden="true">4.2.</strong> Templates</a></li><li><ol class="section"><li class="chapter-item expanded "><a href="user_guide/templates/values.html"><strong aria-hidden="true">4.2.1.</strong> Values</a></li><li class="chapter-item expanded "><a href="user_guide/templates/functions.html"><strong aria-hidden="true">4.2.2.</strong> Functions</a></li><li class="chapter-item expanded "><a href="user_guide/templates/examples.html"><strong aria-hidden="true">4.2.3.</strong> Examples</a></li></ol></li><li class="chapter-item expanded "><a href="user_guide/profiles.html"><strong aria-hidden="true">4.3.</strong> Profiles</a></li></ol></li><li class="chapter-item expanded "><a href="user_guide/composition.html"><strong aria-hidden="true">5.</strong> Collection Reuse with $ref</a></li><li class="chapter-item expanded "><a href="user_guide/streaming.html"><strong aria-hidden="true">6.</strong> Data Streaming &amp; File Upload</a></li><li class="chapter-item expanded "><a href="user_guide/import.html"><strong aria-hidden="true">7.</strong> Importing External Collections</a></li><li class="chapter-item expanded "><a href="user_guide/tui/index.html"><strong aria-hidden="true">8.</strong> Terminal User Interface (TUI)</a></li><li><ol class="section"><li class="chapter-item expanded "><a href="user_guide/tui/editor.html"><strong aria-hidden="true">8.1.</strong> In-App Editing &amp; File Viewing</a></li><li class="chapter-item expanded "><a href="user_guide/tui/filter_query.html"><strong aria-hidden="true">8.2.</strong> Data Filtering &amp; Querying</a></li></ol></li><li class="chapter-item expanded "><a href="user_guide/cli/index.html"><strong aria-hidden="true">9.</strong> Command Line Interface (CLI)</a></li><li><ol class="section"><li class="chapter-item expanded "><a href="user_guide/cli/subcommands.html"><strong aria-hidden="true">9.1.</strong> Subcommands</a></li></ol></li><li class="chapter-item expanded "><a href="user_guide/database.html"><strong aria-hidden="true">10.</strong> Database &amp; Persistence</a></li><li class="chapter-item expanded "><a href="user_guide/json_schema.html"><strong aria-hidden="true">11.</strong> JSON Schema: Completion &amp; Validation</a></li><li class="chapter-item expanded affix "><li class="part-title">API Reference</li><li class="chapter-item expanded "><a href="api/request_collection/index.html"><strong aria-hidden="true">12.</strong> Request Collection</a></li><li><ol class="section"><li class="chapter-item expanded "><a href="api/request_collection/profile.html"><strong aria-hidden="true">12.1.</strong> Profile</a></li><li class="chapter-item expanded "><a href="api/request_collection/request_recipe.html"><strong aria-hidden="true">12.2.</strong> Request Recipe</a></li><li><ol class="section"><li class="chapter-item expanded "><a href="api/request_collection/query_parameters.html"><strong aria-hidden="true">12.2.1.</strong> Query Parameters</a></li><li class="chapter-item expanded "><a href="api/request_collection/authentication.html"><strong aria-hidden="true">12.2.2.</strong> Authentication</a></li><li class="chapter-item expanded "><a href="api/request_collection/recipe_body.html"><strong aria-hidden="true">12.2.3.</strong> Recipe Body</a></li></ol></li></ol></li><li class="chapter-item expanded "><a href="api/configuration/index.html"><strong aria-hidden="true">13.</strong> Configuration</a></li><li><ol class="section"><li class="chapter-item expanded "><a href="api/configuration/input_bindings.html"><strong aria-hidden="true">13.1.</strong> Input Bindings</a></li><li class="chapter-item expanded "><a href="api/configuration/mime.html"><strong aria-hidden="true">13.2.</strong> MIME Maps</a></li><li class="chapter-item expanded "><a href="api/configuration/theme.html"><strong aria-hidden="true">13.3.</strong> Theme</a></li></ol></li><li class="chapter-item expanded "><a href="api/template_functions.html"><strong aria-hidden="true">14.</strong> Template Functions</a></li><li class="chapter-item expanded affix "><li class="part-title">Integration</li><li class="chapter-item expanded "><a href="integration/python.html"><strong aria-hidden="true">15.</strong> Python</a></li><li class="chapter-item expanded "><a href="integration/neovim-integration.html"><strong aria-hidden="true">16.</strong> Neovim Integration</a></li><li class="chapter-item expanded affix "><li class="part-title">Troubleshooting</li><li class="chapter-item expanded "><a href="troubleshooting/logs.html"><strong aria-hidden="true">17.</strong> Logs</a></li><li class="chapter-item expanded "><a href="troubleshooting/lost_history.html"><strong aria-hidden="true">18.</strong> Lost Request History</a></li><li class="chapter-item expanded "><a href="troubleshooting/tls.html"><strong aria-hidden="true">19.</strong> TLS Certificate Errors</a></li><li class="chapter-item expanded affix "><li class="part-title">Other</li><li class="chapter-item expanded "><a href="other/v4_migration.html"><strong aria-hidden="true">20.</strong> v3 to v4 Migration</a></li><li class="chapter-item expanded "><a href="other/changelog.html"><strong aria-hidden="true">21.</strong> Changelog</a></li></ol>';
        // Set the current, active page, and reveal it if it's hidden
        let current_page = document.location.href.toString().split("#")[0].split("?")[0];
        if (current_page.endsWith("/")) {
            current_page += "index.html";
        }
        var links = Array.prototype.slice.call(this.querySelectorAll("a"));
        var l = links.length;
        for (var i = 0; i < l; ++i) {
            var link = links[i];
            var href = link.getAttribute("href");
            if (href && !href.startsWith("#") && !/^(?:[a-z+]+:)?\/\//.test(href)) {
                link.href = path_to_root + href;
            }
            // The "index" page is supposed to alias the first chapter in the book.
            if (link.href === current_page || (i === 0 && path_to_root === "" && current_page.endsWith("/index.html"))) {
                link.classList.add("active");
                var parent = link.parentElement;
                if (parent && parent.classList.contains("chapter-item")) {
                    parent.classList.add("expanded");
                }
                while (parent) {
                    if (parent.tagName === "LI" && parent.previousElementSibling) {
                        if (parent.previousElementSibling.classList.contains("chapter-item")) {
                            parent.previousElementSibling.classList.add("expanded");
                        }
                    }
                    parent = parent.parentElement;
                }
            }
        }
        // Track and set sidebar scroll position
        this.addEventListener('click', function(e) {
            if (e.target.tagName === 'A') {
                sessionStorage.setItem('sidebar-scroll', this.scrollTop);
            }
        }, { passive: true });
        var sidebarScrollTop = sessionStorage.getItem('sidebar-scroll');
        sessionStorage.removeItem('sidebar-scroll');
        if (sidebarScrollTop) {
            // preserve sidebar scroll position when navigating via links within sidebar
            this.scrollTop = sidebarScrollTop;
        } else {
            // scroll sidebar to current active section when navigating via "next/previous chapter" buttons
            var activeSection = document.querySelector('#sidebar .active');
            if (activeSection) {
                activeSection.scrollIntoView({ block: 'center' });
            }
        }
        // Toggle buttons
        var sidebarAnchorToggles = document.querySelectorAll('#sidebar a.toggle');
        function toggleSection(ev) {
            ev.currentTarget.parentElement.classList.toggle('expanded');
        }
        Array.from(sidebarAnchorToggles).forEach(function (el) {
            el.addEventListener('click', toggleSection);
        });
    }
}
window.customElements.define("mdbook-sidebar-scrollbox", MDBookSidebarScrollbox);
