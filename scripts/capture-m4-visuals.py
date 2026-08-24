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
        page.get_by_role("treeitem", name="Edge Gateway 10.24.8.11:22").click()
        page.get_by_role("button", name="工具", exact=True).click()
        page.get_by_role("button", name="启动", exact=True).click()
        page.get_by_text("采样中", exact=True).wait_for()
        page.get_by_role("button", name="运行", exact=True).click()
        page.get_by_text("1234", exact=True).wait_for()
        page.screenshot(
            path=str(ASSET_DIRECTORY / "m4-tools-monitor-desktop.png"),
            full_page=True,
        )
        wide = page.evaluate(
            """() => ({
                clientWidth: document.documentElement.clientWidth,
                scrollWidth: document.documentElement.scrollWidth,
                metricCards: document.querySelectorAll('.monitor-tool > .metric-grid > div').length,
                processRows: document.querySelectorAll('.probe-rows > div').length,
                sampleLabel: document.querySelector('.monitor-status small')?.textContent,
                toolTabs: document.querySelectorAll('.tools-tabs button').length
            })"""
        )
        print(f"WIDE={wide}")
        assert wide == {
            "clientWidth": 1440,
            "scrollWidth": 1440,
            "metricCards": 2,
            "processRows": 1,
            "sampleLabel": "1 / 300 点 · 5s",
            "toolTabs": 4,
        }

        page.get_by_role("button", name="详情", exact=True).click()
        assert page.locator(".tools-workspace-shell").get_attribute("hidden") == ""
        page.get_by_role("button", name="工具", exact=True).click()
        page.get_by_text("采样中", exact=True).wait_for()
        assert page.locator(".monitor-status small").text_content() == "1 / 300 点 · 5s"

        page.set_viewport_size({"width": 1100, "height": 780})
        page.get_by_role("button", name="片段", exact=True).click()
        page.get_by_text("查看服务日志", exact=True).wait_for()
        page.wait_for_timeout(200)
        page.screenshot(
            path=str(ASSET_DIRECTORY / "m4-tools-snippets-narrow.png"),
            full_page=True,
        )
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
        browser.close()


if __name__ == "__main__":
    main()
