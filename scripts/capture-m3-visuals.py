"""Capture reproducible M3 Operations workspace visual evidence."""

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
        print("BUTTONS=" + " | ".join(page.get_by_role("button").all_inner_texts()[:30]))
        page.get_by_role("button", name="SFTP", exact=True).click()
        page.get_by_role("button", name="连接 SFTP").click()
        page.get_by_text("README.txt").wait_for()
        page.screenshot(
            path=str(ASSET_DIRECTORY / "m3-operations-desktop.png"), full_page=True
        )
        wide = page.evaluate(
            """() => ({
                clientWidth: document.documentElement.clientWidth,
                scrollWidth: document.documentElement.scrollWidth,
                rows: document.querySelectorAll('.sftp-row').length,
                hint: document.querySelector('.sftp-drop-hint')?.textContent
            })"""
        )
        print(f"WIDE={wide}")

        page.set_viewport_size({"width": 390, "height": 844})
        page.wait_for_timeout(300)
        page.screenshot(
            path=str(ASSET_DIRECTORY / "m3-operations-mobile.png"), full_page=True
        )
        narrow = page.evaluate(
            """() => ({
                clientWidth: document.documentElement.clientWidth,
                scrollWidth: document.documentElement.scrollWidth,
                panelWidth: document.querySelector('.operations-panel')?.getBoundingClientRect().width,
                contentScrollWidth: document.querySelector('.operations-content')?.scrollWidth,
                contentClientWidth: document.querySelector('.operations-content')?.clientWidth
            })"""
        )
        print(f"NARROW={narrow}")
        browser.close()


if __name__ == "__main__":
    main()
