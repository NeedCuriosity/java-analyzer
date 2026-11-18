import java.util.List;

class Main {
    int run(List<String> names) {
        return names./*@lib_method*/size() + "x"./*@lib_method2*/length();
    }

    @Override
    public String /*@override_lib*/toString() { return "main"; }
}
//? rename lib_method count ; fails
//? rename lib_method2 len ; fails
//? rename override_lib describe ; fails
