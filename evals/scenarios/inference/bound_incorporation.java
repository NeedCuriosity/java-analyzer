import java.util.List;
import java.util.concurrent.Callable;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.CompletionStage;
import java.util.concurrent.Future;
import java.util.function.Supplier;

interface Limiter {
    // `T` 只出现在 `F` 的上界里：由 `F` 的绑定推出
    <T, F extends Future<T>> Callable<T> decorate(Supplier<F> futureSupplier);

    <T, F extends CompletionStage<T>> Supplier<CompletionStage<T>> decorateStage(Supplier<F> supplier);

    default <T, F extends Future<T>> T execute(Supplier<F> futureSupplier) throws Exception {
        return decorate(futureSupplier).call();
    }

    static <E, L extends List<E>> E first(L list) {
        return list.get(0);
    }
}

class Use {
    void run(Limiter limiter, Future<Integer> future, List<String> names) throws Exception {
        Supplier<CompletableFuture<String>> strings = () -> CompletableFuture.completedFuture("x");
        String text = limiter.decorate(strings).call();
        Supplier<Future<Integer>> numbers = () -> future;
        int number = limiter.execute(numbers);
        int again = limiter.decorate(numbers).call();
        Supplier<CompletionStage<Integer>> stages = () -> CompletableFuture.completedFuture(1);
        int staged = limiter.decorateStage(stages).get().toCompletableFuture().get();
        var /*@name*/name = Limiter.first(names);
        String upper = name.toUpperCase();
    }
}
//? hover name ~ "String name"
//? diag none
