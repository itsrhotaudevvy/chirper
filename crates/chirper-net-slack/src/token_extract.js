new Promise(resolve => {
	let mautrixSlackTokenCheckInterval
	let useSlackInBrowserClicked = false
	function mautrixFindSlackToken() {
		// Automatically click the "Use Slack in Browser" button
		if (/\.slack\.com$/.test(window.location.host)) {
			const link = document?.querySelector?.(".p-ssb_redirect__body")?.querySelector?.(".c-link")
			if (link && !useSlackInBrowserClicked) {
				location.href = link.getAttribute("href")
				useSlackInBrowserClicked = true
			}
		}
		if (!localStorage.localConfig_v2?.includes("xoxc-")) {
			return
		}
		const auth_token = Object.values(JSON.parse(localStorage.localConfig_v2).teams)[0].token
		window.clearInterval(mautrixSlackTokenCheckInterval)
		resolve({ auth_token })
	}
	mautrixSlackTokenCheckInterval = window.setInterval(mautrixFindSlackToken, 1000)
})
