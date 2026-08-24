"""Capture reproducible M3 supplement UI evidence without real credentials."""

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
        page.get_by_role("textbox", name="远程地址").wait_for()
        page.wait_for_function(
            "document.querySelector('[aria-label=\"远程地址\"]')?.value === '/home/preview'"
        )
        page.screenshot(
            path=str(ASSET_DIRECTORY / "m3-supplement-sftp-desktop.png"),
            full_page=True,
        )
        wide = page.evaluate(
            """() => {
              const iconControls = [...document.querySelectorAll('.sftp-toolbar > button.icon-button, .sftp-toolbar .sftp-options summary')];
              const iconCenterError = Math.max(...iconControls.map((control) => {
                const icon = control.querySelector('svg');
                const controlBox = control.getBoundingClientRect();
                const iconBox = icon.getBoundingClientRect();
                return Math.abs(controlBox.left + controlBox.width / 2 - iconBox.left - iconBox.width / 2);
              }));
              return ({
                clientWidth: document.documentElement.clientWidth,
                scrollWidth: document.documentElement.scrollWidth,
                address: document.querySelector('[aria-label="远程地址"]')?.value,
                treeItems: document.querySelectorAll('.sftp-tree [role="treeitem"]').length,
                fileRows: document.querySelectorAll('.sftp-table .sftp-row').length,
                toolbarHeight: document.querySelector('.sftp-toolbar')?.getBoundingClientRect().height,
                rootBackground: getComputedStyle(document.querySelector('.sftp-tree-root button:last-child')).backgroundColor,
                iconCenterError
              });
            }"""
        )
        print(f"WIDE={wide}")
        assert wide == {
            "clientWidth": 1440,
            "scrollWidth": 1440,
            "address": "/home/preview",
            "treeItems": 2,
            "fileRows": 3,
            "toolbarHeight": 33,
            "rootBackground": "rgb(220, 233, 224)",
            "iconCenterError": 0,
        }

        page.get_by_role("button", name="工作区配置").click()
        dialog = page.get_by_role("dialog", name="工作区配置")
        dialog.get_by_role("button", name="终端").click()
        dialog.get_by_role("button", name="编辑 Default").click()
        dialog.locator(".entity-form select").first.select_option("custom")
        dialog.get_by_label("终端主题预览").wait_for()
        page.screenshot(
            path=str(ASSET_DIRECTORY / "m3-supplement-theme-editor.png"),
            full_page=True,
        )
        theme = page.evaluate(
            """() => ({
                colors: document.querySelectorAll('.palette-grid input[type="color"]').length,
                preview: document.querySelector('.terminal-theme-preview')?.textContent
            })"""
        )
        print(f"THEME={theme}")
        assert theme["colors"] == 21
        assert "FirstShell" in theme["preview"]
        dialog.get_by_role("button", name="关闭").click()

        page.set_viewport_size({"width": 390, "height": 844})
        page.wait_for_timeout(300)
        page.screenshot(
            path=str(ASSET_DIRECTORY / "m3-supplement-sftp-mobile.png"),
            full_page=True,
        )
        narrow = page.evaluate(
            """() => ({
                clientWidth: document.documentElement.clientWidth,
                scrollWidth: document.documentElement.scrollWidth,
                operationsClientWidth: document.querySelector('.operations-content')?.clientWidth,
                operationsScrollWidth: document.querySelector('.operations-content')?.scrollWidth,
                treeWidth: document.querySelector('.sftp-tree')?.getBoundingClientRect().width
            })"""
        )
        print(f"NARROW={narrow}")
        assert narrow == {
            "clientWidth": 390,
            "scrollWidth": 390,
            "operationsClientWidth": 390,
            "operationsScrollWidth": 560,
            "treeWidth": 190,
        }
        browser.close()


if __name__ == "__main__":
    main()
