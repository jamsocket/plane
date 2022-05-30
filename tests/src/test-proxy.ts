import anyTest, { TestFn } from 'ava';
import { TestEnvironment } from './environment';
import { DroneRunner } from './runner';

const test = anyTest as TestFn<TestEnvironment>;

test.before(async (t) => {
    await DroneRunner.build()
})

test.beforeEach(async (t) => {
    t.context = await TestEnvironment.create()
})

test.afterEach(async (t) => {
    await t.context.drop()
})

test("proxy-unrecognized-host", async (t) => {
    let proxyPort = await t.context.runner.serve()
    
    let result = await fetch(`http://127.0.0.1:${proxyPort}/`,
        { headers: { 'host': 'foo.bar' } })
    
    t.is(result.status, 404)
})
