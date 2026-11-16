import java.util.function.Consumer;

interface BodySpec<B, S extends BodySpec<B, S>> {
    <T extends S> T value(Consumer<B> consumer);
    <T extends S> T isEqualTo(B expected);
    String returnResult();
}

interface ResponseSpec {
    <B> BodySpec<B, ?> expectBody(Class<B> type);
}

class Main {
    String run(ResponseSpec spec) {
        spec.expectBody(String.class).value(body -> body.length())./*@chain*/returnResult();
        spec.expectBody(String.class)./*@stmt*/value(body -> body.isEmpty());
        return spec.expectBody(String.class).isEqualTo("x")./*@chain2*/returnResult();
    }
}
//? hover chain ~ "String returnResult()"
//? complete chain2 has returnResult value isEqualTo
//? hover stmt ~ "<T extends S> T value(Consumer<B> consumer)"
//? diag none
