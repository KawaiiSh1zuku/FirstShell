"""Capture reproducible credential and connection-menu UI evidence."""

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
        page.get_by_role("button", name="浅色").click()
        connection = page.get_by_role(
            "treeitem", name="Edge Gateway 10.24.8.11:22"
        )
        connection.click(button="right")
        menu = page.get_by_role("menu", name="Edge Gateway 操作")
        menu.wait_for()
        page.screenshot(
            path=str(ASSET_DIRECTORY / "connection-context-menu-light.png"),
            full_page=True,
        )
        menu_metrics = menu.evaluate(
            """element => ({
                items: element.querySelectorAll('[role="menuitem"]').length,
                background: getComputedStyle(element).backgroundColor,
                width: element.getBoundingClientRect().width
            })"""
        )
        print(f"MENU={menu_metrics}")
        assert menu_metrics["items"] == 2
        assert menu_metrics["background"] == "rgb(233, 237, 234)"
        assert menu_metrics["width"] >= 142

        menu.get_by_role("menuitem", name="编辑连接").click()
        dialog = page.get_by_role("dialog", name="编辑连接")
        dialog.get_by_label("认证").select_option("private_key")
        passphrase = dialog.locator('input[type="password"]')
        passphrase.wait_for()
        page.screenshot(
            path=str(ASSET_DIRECTORY / "connection-private-key-editor-light.png"),
            full_page=True,
        )
        editor_metrics = dialog.evaluate(
            """element => ({
                passwordType: element.querySelector('input[autocomplete="new-password"]')?.type,
                background: getComputedStyle(element).backgroundColor,
                viewportOverflow: document.documentElement.scrollWidth - document.documentElement.clientWidth
            })"""
        )
        editor_metrics["authentication"] = dialog.get_by_label("认证").input_value()
        print(f"EDITOR={editor_metrics}")
        assert editor_metrics == {
            "authentication": "private_key",
            "passwordType": "password",
            "background": "rgb(250, 251, 250)",
            "viewportOverflow": 0,
        }
        browser.close()


if __name__ == "__main__":
    main()
