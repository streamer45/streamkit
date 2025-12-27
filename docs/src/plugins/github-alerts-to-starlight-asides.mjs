// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

import { visit } from 'unist-util-visit';
import { h, s } from 'hastscript';

/**
 * Rehype plugin to convert GitHub-style alert callouts into Starlight asides.
 *
 * Input HTML:
 *   <blockquote><p>[!NOTE]<br>Some text</p></blockquote>
 *
 * Output HTML:
 *   <aside class="starlight-aside starlight-aside--note" aria-label="Note">
 *     <p class="starlight-aside__title" aria-hidden="true">...</p>
 *     <section class="starlight-aside__content"><p>Some text</p></section>
 *   </aside>
 */
export default function githubAlertsToStarlightAsides() {
	/** @type {Record<string, { variant: string, title: string }>} */
	const typeMap = {
		NOTE: { variant: 'note', title: 'Note' },
		TIP: { variant: 'tip', title: 'Tip' },
		IMPORTANT: { variant: 'note', title: 'Important' },
		WARNING: { variant: 'danger', title: 'Warning' },
		CAUTION: { variant: 'caution', title: 'Caution' },
	};

	// SVG icon paths for each variant
	const iconPaths = {
		note: 'M12 11C11.7348 11 11.4804 11.1054 11.2929 11.2929C11.1054 11.4804 11 11.7348 11 12V16C11 16.2652 11.1054 16.5196 11.2929 16.7071C11.4804 16.8946 11.7348 17 12 17C12.2652 17 12.5196 16.8946 12.7071 16.7071C12.8946 16.5196 13 16.2652 13 16V12C13 11.7348 12.8946 11.4804 12.7071 11.2929C12.5196 11.1054 12.2652 11 12 11ZM12.38 7.08C12.1365 6.97998 11.8635 6.97998 11.62 7.08C11.4973 7.12759 11.3851 7.19896 11.29 7.29C11.2017 7.3872 11.1306 7.49882 11.08 7.62C11.024 7.73868 10.9966 7.86882 11 8C10.9992 8.13161 11.0245 8.26207 11.0742 8.38391C11.124 8.50574 11.1973 8.61656 11.29 8.71C11.3872 8.79833 11.4988 8.86936 11.62 8.92C11.7715 8.98224 11.936 9.00632 12.099 8.99011C12.2619 8.97391 12.4184 8.91792 12.5547 8.82707C12.691 8.73622 12.8029 8.61328 12.8805 8.46907C12.9582 8.32486 12.9992 8.16378 13 8C12.9963 7.73523 12.8927 7.48163 12.71 7.29C12.6149 7.19896 12.5028 7.12759 12.38 7.08ZM12 2C10.0222 2 8.08879 2.58649 6.4443 3.6853C4.79981 4.78412 3.51809 6.3459 2.76121 8.17317C2.00433 10.0004 1.8063 12.0111 2.19215 13.9509C2.578 15.8907 3.53041 17.6725 4.92894 19.0711C6.32746 20.4696 8.10929 21.422 10.0491 21.8079C11.9889 22.1937 13.9996 21.9957 15.8268 21.2388C17.6541 20.4819 19.2159 19.2002 20.3147 17.5557C21.4135 15.9112 22 13.9778 22 12C22 10.6868 21.7413 9.38642 21.2388 8.17317C20.7363 6.95991 19.9997 5.85752 19.0711 4.92893C18.1425 4.00035 17.0401 3.26375 15.8268 2.7612C14.6136 2.25866 13.3132 2 12 2ZM12 20C10.4178 20 8.87104 19.5308 7.55544 18.6518C6.23985 17.7727 5.21447 16.5233 4.60897 15.0615C4.00347 13.5997 3.84504 11.9911 4.15372 10.4393C4.4624 8.88743 5.22433 7.46197 6.34315 6.34315C7.46197 5.22433 8.88743 4.4624 10.4393 4.15372C11.9911 3.84504 13.5997 4.00346 15.0615 4.60896C16.5233 5.21447 17.7727 6.23984 18.6518 7.55544C19.5308 8.87103 20 10.4177 20 12C20 14.1217 19.1572 16.1566 17.6569 17.6569C16.1566 19.1571 14.1217 20 12 20Z',
		tip: 'M12 18C11.4696 18 10.9609 18.2107 10.5858 18.5858C10.2107 18.9609 10 19.4696 10 20C10 20.5304 10.2107 21.0391 10.5858 21.4142C10.9609 21.7893 11.4696 22 12 22C12.5304 22 13.0391 21.7893 13.4142 21.4142C13.7893 21.0391 14 20.5304 14 20C14 19.4696 13.7893 18.9609 13.4142 18.5858C13.0391 18.2107 12.5304 18 12 18ZM19 11V8C19 5.24 16.76 3 14 3H10C7.24 3 5 5.24 5 8V11C5 13.76 7.24 16 10 16H14C16.76 16 19 13.76 19 11ZM17 11C17 12.6569 15.6569 14 14 14H10C8.34315 14 7 12.6569 7 11V8C7 6.34315 8.34315 5 10 5H14C15.6569 5 17 6.34315 17 8V11Z',
		caution:
			'M12 16C11.7348 16 11.4804 16.1054 11.2929 16.2929C11.1054 16.4804 11 16.7348 11 17C11 17.2652 11.1054 17.5196 11.2929 17.7071C11.4804 17.8946 11.7348 18 12 18C12.2652 18 12.5196 17.8946 12.7071 17.7071C12.8946 17.5196 13 17.2652 13 17C13 16.7348 12.8946 16.4804 12.7071 16.2929C12.5196 16.1054 12.2652 16 12 16ZM12 8C11.7348 8 11.4804 8.10536 11.2929 8.29289C11.1054 8.48043 11 8.73478 11 9V13C11 13.2652 11.1054 13.5196 11.2929 13.7071C11.4804 13.8946 11.7348 14 12 14C12.2652 14 12.5196 13.8946 12.7071 13.7071C12.8946 13.5196 13 13.2652 13 13V9C13 8.73478 12.8946 8.48043 12.7071 8.29289C12.5196 8.10536 12.2652 8 12 8ZM12 2C11.8557 2.0003 11.7134 2.03395 11.585 2.098L2.585 6.598C2.43949 6.67049 2.31729 6.78168 2.23161 6.9196C2.14594 7.05752 2.10019 7.21684 2.09961 7.37936C2.09903 7.54188 2.14364 7.70152 2.22833 7.84005C2.31302 7.97858 2.43442 8.09064 2.579 8.164L4 8.87V14.118C4 14.3824 4.10536 14.6359 4.293 14.823L11.293 21.703C11.4805 21.8905 11.7348 21.9959 12 21.9959C12.2652 21.9959 12.5195 21.8905 12.707 21.703L19.707 14.823C19.8946 14.6359 20 14.3824 20 14.118V8.87L21.421 8.164C21.5656 8.09064 21.687 7.97858 21.7717 7.84005C21.8564 7.70152 21.901 7.54188 21.9004 7.37936C21.8998 7.21684 21.8541 7.05752 21.7684 6.9196C21.6827 6.78168 21.5605 6.67049 21.415 6.598L12.415 2.098C12.2866 2.03395 12.1443 2.0003 12 2ZM6 9.7L11.585 12.4C11.7134 12.4641 11.8557 12.4978 12 12.4978C12.1443 12.4978 12.2866 12.4641 12.415 12.4L18 9.7V13.699L12 19.579L6 13.699V9.7ZM12 10.377L5.073 7L12 4.123L18.927 7L12 10.377Z',
		danger: 'M12 7C11.7348 7 11.4804 7.10536 11.2929 7.29289C11.1054 7.48043 11 7.73478 11 8V12C11 12.2652 11.1054 12.5196 11.2929 12.7071C11.4804 12.8946 11.7348 13 12 13C12.2652 13 12.5196 12.8946 12.7071 12.7071C12.8946 12.5196 13 12.2652 13 12V8C13 7.73478 12.8946 7.48043 12.7071 7.29289C12.5196 7.10536 12.2652 7 12 7ZM12 15C11.7348 15 11.4804 15.1054 11.2929 15.2929C11.1054 15.4804 11 15.7348 11 16C11 16.2652 11.1054 16.5196 11.2929 16.7071C11.4804 16.8946 11.7348 17 12 17C12.2652 17 12.5196 16.8946 12.7071 16.7071C12.8946 16.5196 13 16.2652 13 16C13 15.7348 12.8946 15.4804 12.7071 15.2929C12.5196 15.1054 12.2652 15 12 15ZM21.71 7.56L16.44 2.29C16.2526 2.10375 15.9992 1.99921 15.735 2H8.265C8.00078 1.99921 7.74744 2.10375 7.56 2.29L2.29 7.56C2.10375 7.74744 1.99921 8.00078 2 8.265V15.735C1.99921 15.9992 2.10375 16.2526 2.29 16.44L7.56 21.71C7.74744 21.8963 8.00078 22.0008 8.265 22H15.735C15.9992 22.0008 16.2526 21.8963 16.44 21.71L21.71 16.44C21.8963 16.2526 22.0008 15.9992 22 15.735V8.265C22.0008 8.00078 21.8963 7.74744 21.71 7.56ZM20 15.32L15.32 20H8.68L4 15.32V8.68L8.68 4H15.32L20 8.68V15.32Z',
	};

	/**
	 * Create an SVG icon element as a proper hast node.
	 * @param {string} variant
	 * @returns {object}
	 */
	function createIcon(variant) {
		return s(
			'svg',
			{
				viewBox: '0 0 24 24',
				width: '16',
				height: '16',
				fill: 'currentColor',
				class: 'starlight-aside__icon',
				'aria-hidden': 'true',
			},
			[s('path', { d: iconPaths[variant] })]
		);
	}

	/**
	 * Remove the alert marker from the first paragraph.
	 * @param {any} node
	 * @returns {string | null} The marker type if found
	 */
	function extractAndRemoveMarker(node) {
		if (node.tagName !== 'p' || !Array.isArray(node.children)) return null;
		if (node.children.length === 0) return null;

		const first = node.children[0];
		if (first.type !== 'text' || typeof first.value !== 'string') return null;

		const match = first.value.match(/^\s*\[!([A-Z]+)\]\s*/);
		if (!match) return null;

		// Remove the marker from the text
		first.value = first.value.slice(match[0].length);

		// If it starts with a newline after the marker, remove that too
		if (first.value.startsWith('\n')) {
			first.value = first.value.slice(1);
		}

		// If the text node is now empty, remove it
		if (first.value === '') {
			node.children.shift();
			// Also remove any leading <br> element
			if (node.children[0]?.tagName === 'br') {
				node.children.shift();
			}
		}

		return match[1];
	}

	/**
	 * Check if a paragraph is effectively empty (only whitespace/breaks).
	 * @param {any} node
	 * @returns {boolean}
	 */
	function isEmptyParagraph(node) {
		if (node.tagName !== 'p') return false;
		if (!Array.isArray(node.children)) return true;
		return node.children.every(
			(child) =>
				child.tagName === 'br' ||
				(child.type === 'text' && child.value.trim() === '')
		);
	}

	return function transformer(tree) {
		visit(tree, 'element', (node, index, parent) => {
			if (node.tagName !== 'blockquote') return;
			if (!Array.isArray(node.children)) return;

			// Find the first paragraph child
			const firstParagraphIndex = node.children.findIndex(
				(child) => child.type === 'element' && child.tagName === 'p'
			);
			if (firstParagraphIndex === -1) return;

			const firstParagraph = node.children[firstParagraphIndex];
			const markerType = extractAndRemoveMarker(firstParagraph);
			if (!markerType) return;

			const mapped = typeMap[markerType];
			if (!mapped) return;

			// Remove the paragraph if it's now empty
			let contentChildren = [...node.children];
			if (isEmptyParagraph(firstParagraph)) {
				contentChildren.splice(firstParagraphIndex, 1);
			}

			// Filter out leading/trailing whitespace text nodes
			contentChildren = contentChildren.filter(
				(child) => !(child.type === 'text' && child.value.trim() === '')
			);

			// Create the Starlight aside structure
			const aside = h(
				'aside',
				{
					class: `starlight-aside starlight-aside--${mapped.variant}`,
					'aria-label': mapped.title,
				},
				[
					h(
						'p',
						{
							class: 'starlight-aside__title',
							'aria-hidden': 'true',
						},
						[createIcon(mapped.variant), { type: 'text', value: mapped.title }]
					),
					h(
						'section',
						{ class: 'starlight-aside__content' },
						contentChildren
					),
				]
			);

			// Replace the blockquote with the aside
			parent.children[index] = aside;
		});
	};
}
