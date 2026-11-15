import java.util.List;
import java.util.Optional;
import java.util.OptionalInt;
import java.util.stream.IntStream;

class Main {
    private Optional<String> cached = Optional.empty();

    String unchecked(Optional<String> name) {
        return name./*@param*/get();
    }

    String checked(Optional<String> name) {
        if (name.isPresent()) {
            return name.get();
        }
        return "";
    }

    String early(Optional<String> name) {
        if (name.isEmpty()) {
            return "";
        }
        return name.get();
    }

    String chained(List<String> items) {
        return items.stream().findFirst()./*@chain*/get();
    }

    String of() {
        return Optional.of("x").get();
    }

    int primitive() {
        OptionalInt max = IntStream.of(1, 2).max();
        return max./*@int*/getAsInt();
    }

    String field() {
        return cached./*@field*/get();
    }

    Optional<String> lookup() {
        return cached;
    }

    String repeated() {
        if (lookup().isPresent()) {
            return lookup().get();
        }
        return "";
    }

    String explicit(Optional<String> name) {
        return name.orElseThrow();
    }
}
//? diag optional-get-without-check param
//? diag optional-get-without-check chain
//? diag optional-get-without-check int
//? diag optional-get-without-check field
//? diag only optional-get-without-check
//? assist optional_or_else_throw param ; has "return name.orElseThrow();"
