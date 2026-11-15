import java.util.List;

interface DemoService {
    int createBrand(String name);
    int updateBrand(long id, String name);
    int deleteBrand(long id);
    List<String> listAllBrand();
    List<String> listBrand(int pageNum, int pageSize);
    String getBrand(long id);
}

class Base {
    protected void inherited() {}
}

class DemoController extends Base {
    private DemoService demoService;
    private int pageSize;

    void own() {}

    void run() {
        demoService./*@svc*/;
        this./*@self*/;
        int affected = demoService./*@typed*/;
    }
}
//? complete svc lacks clone finalize
//? complete typed order createBrand hashCode
//? complete typed order listBrand hashCode
//? complete typed order getBrand hashCode
//? complete typed order listAllBrand hashCode
