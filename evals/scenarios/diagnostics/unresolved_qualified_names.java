import java.util.List;

class Main {
    int run(List<String> names) {
        int a = java.util./*@missing_class*/Objectz.hash(names);
        Object b = java./*@missing_package*/utill.List.of();
        int c = java.util.Objects.hash(names);
        java.util.Map.Entry<String, String> e = java.util.Map.entry("k", "v");
        return a + c + names.size() + e.getKey().length() + (b == null ? 1 : 0);
    }
}
//? diag unresolved-name missing_class
//? diag unresolved-name missing_package
//? diag only unresolved-name
