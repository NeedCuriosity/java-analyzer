import java.util.List;

class Main {
    int run(List<String> names) {
        return names./*@lib_method*/size() + "x"./*@lib_method2*/length();
    }
}
//? rename lib_method count ; fails
//? rename lib_method2 len ; fails
