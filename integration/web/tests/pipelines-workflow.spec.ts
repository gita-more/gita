import { test, expect } from '@playwright/test'
import { loginViaApi, createRepoWithFiles, DEFAULT_USER } from './helpers'

/**
 * Pipelines 标签页要能看到来自两种格式的流水线。
 *
 * 曾经的 bug：`GET /pipelines` 只读 `.gita-ci.yml`，于是仓库的 CI 写在
 * `.github/workflows/*.yml` 时，Pipelines 标签是空的，运行记录只能靠猜
 * URL 访问。
 */
test.describe('pipelines 标签页 — 工作流文件', () => {
  test('列出 .github/workflows 下的工作流', async ({ page }) => {
    await loginViaApi(page)
    const owner = DEFAULT_USER.username
    const name = `pipelines-wf-${Date.now()}`

    await createRepoWithFiles(page, name, {
      '.github/workflows/ci.yml': [
        'on: push',
        'jobs:',
        '  test:',
        '    runs-on: ubuntu-latest',
        '    steps:',
        '      - run: echo hi',
        '',
      ].join('\n'),
    })

    // The panel fetches pipelines asynchronously after the page loads,
    // so set up the response listener before navigating: otherwise the
    // response arrives during navigation and the listener misses it.
    const pipelinesResponse = page.waitForResponse(
      (response) =>
        response.url().includes(`/api/repos/${owner}/${name}/pipelines`) &&
        response.status() === 200,
    )
    await page.goto(`/${owner}/${name}/pipelines`)
    await pipelinesResponse

    // The pipeline trigger button only renders once the pipeline list
    // has loaded, so waiting for it also waits for the data to land.
    await expect(
      page.getByRole('button', { name: /运行流水线|Run pipeline/ }),
    ).toBeVisible()

    // The repository does have a pipeline configuration, so "no
    // pipelines configured" must not show.
    await expect(
      page.getByText(/还没有流水线配置|No pipelines yet/),
    ).toHaveCount(0)
  })

  test('两种格式共存时可以选择切换', async ({ page }) => {
    await loginViaApi(page)
    const owner = DEFAULT_USER.username
    const name = `pipelines-both-${Date.now()}`

    await createRepoWithFiles(page, name, {
      '.gita-ci.yml': [
        'stages: [build]',
        'build:',
        '  stage: build',
        '  script:',
        '    - echo hi',
        '',
      ].join('\n'),
      '.github/workflows/ci.yml': [
        'on: push',
        'jobs:',
        '  test:',
        '    runs-on: ubuntu-latest',
        '    steps:',
        '      - run: echo hi',
        '',
      ].join('\n'),
    })

    await page.goto(`/${owner}/${name}/pipelines`)

    const picker = page.getByTestId('pipeline-picker')
    await expect(picker).toBeVisible()
    await expect(picker.getByText('.gita-ci.yml')).toBeVisible()
    await expect(picker.getByText('.github/workflows/ci.yml')).toBeVisible()

    // 切到工作流后，选中的按钮变成 active。
    await picker.getByText('.github/workflows/ci.yml').click()
    await expect(
      picker.locator('button', { hasText: '.github/workflows/ci.yml' }),
    ).toHaveClass(/is-active/)
  })
})
