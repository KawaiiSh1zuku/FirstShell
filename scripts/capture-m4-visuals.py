"""Capture reproducible M4 efficiency-tool evidence without real credentials."""

from pathlib import Path

from playwright.sync_api import sync_playwright


PROJECT_ROOT = Path(__file__).resolve().parent.parent
ASSET_DIRECTORY = PROJECT_ROOT / "docs" / "verification" / "assets"
PREVIEW_URL = "http://127.0.0.1:1420/"


def main() -> None:
    ASSET_DIRECTORY.mkdir(parents=True, exist_ok=True)
    with sync_playwright() as playwright:
        browser = playwright.chromium.launch(headless=True)
        page = browser.new_page(
            viewport={"width": 1440, "height": 900}, device_scale_factor=1
        )
        page.goto(PREVIEW_URL, wait_until="networkidle")
        page.get_by_role("button", name="浅色", exact=True).click()
        page.get_by_role("button", name="服务器", exact=True).click()
        page.get_by_role("tree", name="连接树", exact=True).wait_for()
        page.screenshot(
            path=str(ASSET_DIRECTORY / "m4-connections-page-desktop.png"),
            full_page=True,
        )
        connections_page = page.evaluate(
            """() => ({
                clientWidth: document.documentElement.clientWidth,
                scrollWidth: document.documentElement.scrollWidth,
                treeItems: document.querySelectorAll('[role="treeitem"]').length,
                inspectorLeft: Math.round(document.querySelector('.inspector-panel')?.getBoundingClientRect().left ?? -1),
                inspectorWidth: Math.round(document.querySelector('.inspector-panel')?.getBoundingClientRect().width ?? 0),
                pageLeft: Math.round(document.querySelector('.connections-page')?.getBoundingClientRect().left ?? 0),
                serverPressed: document.querySelector('[aria-label="服务器"]')?.getAttribute('aria-pressed')
            })"""
        )
        print(f"CONNECTIONS_PAGE={connections_page}")
        assert connections_page == {
            "clientWidth": 1440,
            "scrollWidth": 1440,
            "treeItems": 5,
            "inspectorLeft": 0,
            "inspectorWidth": 288,
            "pageLeft": 293,
            "serverPressed": "true",
        }

        page.get_by_role("treeitem", name="Edge Gateway 10.24.8.11:22").click()
        page.get_by_text("监控中", exact=True).wait_for()
        page.screenshot(
            path=str(ASSET_DIRECTORY / "m4-tools-monitor-desktop.png"),
            full_page=True,
        )
        wide = page.evaluate(
            """() => ({
                clientWidth: document.documentElement.clientWidth,
                scrollWidth: document.documentElement.scrollWidth,
                metricCards: document.querySelectorAll('.host-metric-grid > div').length,
                diskRows: document.querySelectorAll('.disk-row').length,
                interfaceOptions: document.querySelectorAll('[aria-label="监控网卡"] option').length,
                monitorStatus: document.querySelector('.host-monitor-heading > span')?.textContent,
                inspectorLeft: Math.round(document.querySelector('.inspector-panel')?.getBoundingClientRect().left ?? -1),
                inspectorWidth: Math.round(document.querySelector('.inspector-panel')?.getBoundingClientRect().width ?? 0),
                centerLeft: Math.round(document.querySelector('.center-stack')?.getBoundingClientRect().left ?? 0),
                serverPressed: document.querySelector('[aria-label="服务器"]')?.getAttribute('aria-pressed')
            })"""
        )
        print(f"WIDE={wide}")
        assert wide["clientWidth"] == wide["scrollWidth"] == 1440
        assert wide["metricCards"] == 4
        assert wide["diskRows"] == 1
        assert wide["interfaceOptions"] == 2
        assert wide["monitorStatus"] == "监控中"
        assert wide["inspectorLeft"] == 0
        assert wide["inspectorWidth"] == 288
        assert wide["centerLeft"] == 293
        assert wide["serverPressed"] == "false"

        page.get_by_role("button", name="工具", exact=True).click()
        page.get_by_text("查看服务日志", exact=True).wait_for()
        assert page.locator(".tools-tabs button").count() == 3
        page.get_by_role("button", name="详情", exact=True).click()
        assert page.locator(".tools-workspace-shell").get_attribute("hidden") == ""
        page.get_by_text("监控中", exact=True).wait_for()
        page.get_by_role("button", name="工具", exact=True).click()
        page.get_by_text("查看服务日志", exact=True).wait_for()

        page.set_viewport_size({"width": 1100, "height": 780})
        page.get_by_role("button", name="片段", exact=True).click()
        page.get_by_text("查看服务日志", exact=True).wait_for()
        page.wait_for_timeout(200)
        narrow = page.evaluate(
            """() => ({
                clientWidth: document.documentElement.clientWidth,
                scrollWidth: document.documentElement.scrollWidth,
                inspectorWidth: document.querySelector('.inspector-panel')?.getBoundingClientRect().width,
                snippetCards: document.querySelectorAll('.snippet-list button').length,
                parameterRows: document.querySelectorAll('.parameter-row').length,
                runnerInputs: document.querySelectorAll('.snippet-runner input, .snippet-runner select').length
            })"""
        )
        print(f"NARROW={narrow}")
        assert narrow == {
            "clientWidth": 1100,
            "scrollWidth": 1100,
            "inspectorWidth": 288,
            "snippetCards": 2,
            "parameterRows": 2,
            "runnerInputs": 2,
        }

        page.locator(".snippet-list button").first.click()
        page.wait_for_function(
            "document.querySelector('[aria-label=\"片段名称\"]')?.value === ''"
        )
        new_snippet_layout = page.evaluate(
            """() => ({
                listHeight: Math.round(document.querySelector('.snippet-list')?.getBoundingClientRect().height ?? 0),
                newButtonHeight: Math.round(document.querySelector('.snippet-list button')?.getBoundingClientRect().height ?? 0),
                resolvedTheme: document.documentElement.dataset.resolvedTheme,
                selectedBackground: getComputedStyle(document.querySelector('.snippet-list button.selected')).backgroundColor
            })"""
        )
        print(f"NEW_SNIPPET={new_snippet_layout}")
        assert 0 < new_snippet_layout["listHeight"] <= 40
        assert 0 < new_snippet_layout["newButtonHeight"] <= 40
        assert new_snippet_layout["resolvedTheme"] == "light"
        assert new_snippet_layout["selectedBackground"] != "rgb(37, 53, 43)"
        page.screenshot(
            path=str(ASSET_DIRECTORY / "m4-tools-snippets-narrow.png"),
            full_page=True,
        )

        page.get_by_role("button", name="服务器", exact=True).click()
        page.set_viewport_size({"width": 390, "height": 844})
        page.get_by_role("tree", name="连接树", exact=True).wait_for()
        mobile = page.evaluate(
            """() => ({
                clientWidth: document.documentElement.clientWidth,
                scrollWidth: document.documentElement.scrollWidth,
                pageLeft: Math.round(document.querySelector('.connections-page')?.getBoundingClientRect().left ?? -1),
                pageWidth: Math.round(document.querySelector('.connections-page')?.getBoundingClientRect().width ?? 0),
                inspectorVisible: getComputedStyle(document.querySelector('.inspector-panel')).display !== 'none'
            })"""
        )
        print(f"MOBILE_CONNECTIONS={mobile}")
        assert mobile == {
            "clientWidth": 390,
            "scrollWidth": 390,
            "pageLeft": 0,
            "pageWidth": 390,
            "inspectorVisible": False,
        }
        page.screenshot(
            path=str(ASSET_DIRECTORY / "m4-connections-page-mobile.png"),
            full_page=True,
        )
        browser.close()


if __name__ == "__main__":
    main()
