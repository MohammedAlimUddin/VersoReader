# Verso Reader — UI Refinement and Product Polish Specification

## Objective

Transform the existing Verso Reader application from a functional prototype into a polished, modern, professional PDF reader inspired by Microsoft Edge's PDF interface.

The core PDF functionality is already implemented. This task is primarily about improving the UI/UX, usability, branding, interactions and application integration.

**This is a refinement task, not a rewrite.**

## 1. Mandatory engineering constraints

- Preserve the existing PDF engine, rendering logic, search functionality, annotation behavior and document-management logic wherever possible.
- Do not replace working functionality with mock implementations.
- Do not migrate to another UI framework as part of this task.
- Keep the implementation consistent with the existing architecture.
- Reuse existing components and abstractions when appropriate.
- Avoid introducing unnecessary dependencies.
- Keep the application lightweight and responsive.
- Do not make unrelated changes to the PDF engine.
- Do not claim that a feature works unless it has actually been implemented and tested.

## 2. Overall visual design

Establish a cohesive design system for the entire application.

Requirements:

- Modern, minimal, professional desktop appearance.
- Retain the dark application chrome and white document canvas unless there is a compelling technical reason to change them.
- Use consistent typography, icon sizes, spacing, border radii and control dimensions.
- Use subtle borders and restrained colors.
- Avoid excessive decoration, gradients, shadows and animations.
- Improve visual hierarchy so primary controls are easy to identify.
- Keep the PDF document as the primary visual focus.
- Ensure all controls remain usable at different window sizes and display scaling settings.

Suggested initial design targets:

- Main toolbar height: 42–46 px.
- Tab height: 36–40 px.
- Toolbar icons: 18–20 px.
- Interactive button hit areas: 32–36 px.
- Toolbar spacing: 6–8 px.
- File-path input height: 30–34 px.
- Border radius: approximately 5–7 px.
- Hover and active transitions: approximately 100–160 ms where supported.

Treat these as starting values. Adapt them to the existing framework, platform conventions and actual application layout.

## 3. Toolbar improvements

- Increase toolbar height and button dimensions slightly.
- Improve horizontal spacing between controls.
- Group related controls logically using subtle separators.
- Align icons and labels consistently.
- Ensure the toolbar remains compact enough to maximize document viewing space.
- Provide consistent hover, pressed, focused, disabled and active states.
- Use tooltips for icon-only controls. so the main focus is a great icon and if the user is confused what this icion is then he can use the tooltips
- Make all interactive controls accessible through keyboard navigation where supported.

Suggested logical groups:

1. Sidebar and navigation.
2. Zoom and page controls.
3. Annotation tools.
4. Rotation and document operations.
5. Search and full-screen controls.

Keep Find aligned to the far right of the main toolbar, alongside other secondary actions where appropriate.

## 4. Icon system

Replace the existing inconsistent or visually poor icons with a cohesive SVG icon set.

you can learn from the consistent code of lucide icon set. but customize for my use cases

Requirements:

- Consistent stroke width, proportions and visual weight.
- Crisp rendering at common display scales.
- Consistent alignment within buttons.
- No emoji or mismatched icon styles.
- Reuse existing icons if they meet the quality requirements.
- Prefer a single coherent icon library over individually improvised icons.

Create clear visual states:

- Default: muted gray.
- Hover: brighter foreground or subtle background.
- Active: accent-colored icon and/or tinted background.
- Disabled: visibly muted.
- Keyboard focus: clear focus indicator.

Tool-specific behavior:

- Highlighter: reflect the selected highlight color when appropriate.
- Eraser: use a consistent active-tool accent.
- Text tool: clearly indicate when active.
- Sidebar: indicate whether the sidebar is open.
- Full screen: reflect the current full-screen state.

## 5. Tab bar

- Increase tab height slightly without making the interface bulky.
- Improve tab padding and text readability.
- Clearly distinguish the active tab from inactive tabs.
- Add a subtle hover background or border for inactive tabs.
- Make hover feedback immediate and visually consistent.
- Show the close button on hover, or keep it visible if that better matches the existing design.
- When hovering over the close button, change its icon and/or background to a clear red close state.
- Ensure the close-button hover effect does not accidentally trigger the tab's general hover state.
- Truncate long filenames cleanly.
- Preserve independent document state when switching tabs.
- Add a polished new-tab button.
- Indicate unsaved changes without relying on an ambiguous label.
- replace the current pdf icon on the tab with more polish one, you can create one using svg,

When closing a document with unsaved changes, ask for confirmation before discarding them.

## 6. File-path bar and opening PDFs

- Increase the file-path bar height and padding slightly.
- Improve text readability and contrast.
- Make the path input look like a polished file-location field rather than a debug textbox.
- Keep the path bar collapsible.
- Preserve the current ability to enter or paste a local file path manually.
- Pressing Enter should attempt to open the entered path.
- The Open button (add icon for it and the tooltip should show the open written on it) must open a native file chooser instead of merely focusing the text input.
- Restrict the file chooser to PDF files through the appropriate file-type filter.
- Validate the selected path and show useful errors for inaccessible, missing or invalid files.
- Handle paths containing spaces and other valid filesystem characters correctly.
- Preserve existing open-file behavior and avoid duplicate tabs unless the current application intentionally allows them.

## 7. Find/search interface

- Move the Find control to the far right of the main toolbar.
- Provide a dedicated search panel or popover.
- Include a search input, match count, previous/next controls and close button.
- Support Enter and Shift+Enter for navigating matches.
- Support Esc for closing the search panel.
- Support Ctrl+F for opening search.
- Provide clear visual feedback for the current match and other matches.
- Handle no-results states gracefully.
- Keep search responsive on large PDFs.
- Do not block the UI while searching.
- Preserve existing search-engine functionality.

## 8. Annotation tools

### Highlighter

- Improve the icon and active state.
- Provide a color-selection popover with common highlight colors and a custom color option if supported.
- Display the currently selected color clearly.
- Keep the toolbar visually compact.
- Preserve existing annotation creation and saving behavior.

### Eraser

- Use a clear and consistent icon.
- Make active mode obvious.
- Ensure erasing removes the intended annotation rather than painting over document content.
- Provide appropriate feedback when no annotation is present under the cursor.
- ensure the cursor turn into an eraser icon&#x20;

### Text tool

- Improve the icon and active state.
- Make text-annotation placement and editing intuitive.
- Preserve existing text annotation behavior.

### Undo and redo

- Add undo and redo controls if the architecture supports them reliably.
- Provide Ctrl+Z and Ctrl+Shift+Z shortcuts.
- Ensure undo and redo operate on actual annotation/document changes.
- Disable controls when their operations are unavailable.
- increase the two finger touchpad scroll speed a little

##

##

## 9. Application identity and branding

The application's canonical name must be **Verso Reader**.

Search the entire project for old product names, including PDFView, pdfview and other obsolete branding.

Update all applicable occurrences:

- Window title.
- Application metadata.
- About dialog.
- Desktop entry and launcher metadata.
- Package name and application display name.
- Dock/task-switcher identity where applicable.
- Installer or distribution metadata.
- Relevant user-facing error messages.
- Documentation and README where appropriate.

Do not blindly rename internal identifiers, crate names or module names if doing so would break the project or provide no user-facing benefit. Make any internal renaming deliberate and consistent.

## 10. Application icon and dock integration

- Locate the icon asset supplied in the project directory.
- Inspect its format, dimensions and transparency.
- Use it as the application icon wherever supported.
- Ensure the icon appears in the Ubuntu dock, application launcher and window/task-switcher identity where applicable.
- Update the desktop entry and packaging metadata if necessary.
- Follow the existing framework's platform-specific requirements.
- Do not invent a replacement icon if the supplied asset is valid.
- Verify the result by launching the application in the normal desktop session.

## 14. About dialog

Add an About item in an appropriate location, such as the application menu, settings area or a compact menu.

Clicking About should open a professionally designed modal containing:

- Application icon.
- Verso Reader name.
- Application version.
- Developer name: Mohammed Alim Uddin.
- University: Bangladesh University of Engineering and Technology (BUET).
- A short description, such as "A lightweight desktop PDF reader."
- Optional links to the project repository and issue tracker, if their URLs are known.

Design requirements:

- Consistent typography and spacing.
- Clear hierarchy.
- Appropriate modal sizing.
- A clean close button.
- Keyboard dismissal with Esc.
- Correct focus behavior.
- No invented URLs or unsupported claims.

##

## 16. Performance and responsiveness

- Preserve the existing lightweight architecture.
- Avoid unnecessary dependencies and background threads.
- Ensure UI interactions remain responsive during rendering and search.
- Avoid rendering every PDF page at full resolution.
- Preserve bounded caching and cancellation of obsolete rendering work where already implemented.
- Ensure hover effects and animations do not interfere with input responsiveness.
- Verify that opening multiple tabs does not cause uncontrolled memory growth.

## 17. Testing and acceptance criteria

Before declaring the work complete:

1. Compile the application successfully.
2. Run relevant automated tests.
3. Launch the application and inspect the actual UI.
4. Open a PDF using the native file chooser.
5. Open multiple PDFs and switch between tabs.
6. Verify active, hover and close-button states.
7. Verify the Find panel and search navigation.
8. Test highlighting, erasing, text annotations and saving.
9. Verify page navigation, zoom, rotation and full-screen behavior.
10. Test touchpad scrolling and pinch-to-zoom where supported by the current environment.
11. Verify the application icon and canonical name in the Ubuntu desktop environment.
12. Verify the About modal.
13. Test closing documents with unsaved changes.
14. Check for regressions in existing functionality.
15. Record any unsupported or partially implemented features honestly.

##
