/*{
    "CREDIT": "Shadertoy port",
    "DESCRIPTION": "Volumetric cloud renderer (ported from Shadertoy)",
    "ISFVSN": "2.0",
    "CATEGORIES": ["Generator"],
    "INPUTS": []
}*/

#ifdef GL_ES
precision highp float;
#endif

// --- Shadertoy compatibility shims ---
vec3 iResolution = vec3(RENDERSIZE.x, RENDERSIZE.y, 1.0);
float iTime = TIME;

//
// [0] 纯体积云渲染器 (Shadertoy)
// 功能模块化：通过 [1] 区宏开关控制性能与画质
// 无背景、无倒影、无水平投影
//

// ----------------------------------------------------------------------------
// [1] 质量与性能控制开关 —— 取消注释一个预设，或保持全注释以手动配置
// ----------------------------------------------------------------------------
// #define QUALITY_LOW      // [1.0a] 极速：20步、无体积光、3层噪声、各向同性
//#define QUALITY_MEDIUM      // [1.0b] 平衡：40步、简化体积光、4层噪声
#define QUALITY_HIGH      // [1.0c] 画质：80步、完整体积光、5层噪声、HG相位

// [1.1] 独立开关（当使用上方预设时，以下值会被覆盖；如需手动微调，先取消所有 QUALITY_* 宏）
// 注意：手动修改下方数值前，请确保上方三个 QUALITY_* 宏全都被注释掉
#ifndef QUALITY_LOW
#ifndef QUALITY_MEDIUM
#ifndef QUALITY_HIGH
    #define MARCH_STEPS 50          // [1.1a] 手动：步进数（3~80）
    #define ENABLE_BEER_LAMBERT 1   // [1.1b] 手动：1=Beer-Lambert体积光，0=直接叠加
    #define ENABLE_PHASE_FUNCTION 1 // [1.1c] 手动：1=HG相位函数，0=各向同性
    #define FBM_OCTAVES 5           // [1.1d] 手动：FBM噪声层数（3~5）
    #define ENABLE_EARLY_EXIT 1     // [1.1e] 手动：1=Alpha饱和提前退出
#endif
#endif
#endif

// [1.2] 预设配置（自动填充上方开关，覆盖手动值）
#ifdef QUALITY_LOW
    #define MARCH_STEPS 20
    #define ENABLE_BEER_LAMBERT 0
    #define ENABLE_PHASE_FUNCTION 0
    #define FBM_OCTAVES 3
    #define ENABLE_EARLY_EXIT 1
#endif

#ifdef QUALITY_MEDIUM
    #define MARCH_STEPS 40
    #define ENABLE_BEER_LAMBERT 0
    #define ENABLE_PHASE_FUNCTION 0
    #define FBM_OCTAVES 4
    #define ENABLE_EARLY_EXIT 1
#endif

#ifdef QUALITY_HIGH
    #define MARCH_STEPS 80
    #define ENABLE_BEER_LAMBERT 1
    #define ENABLE_PHASE_FUNCTION 1
    #define FBM_OCTAVES 5
    #define ENABLE_EARLY_EXIT 1
#endif

// ----------------------------------------------------------------------------
// [2] 场景可调参数区 —— 修改这些值改变透视与外观
// ----------------------------------------------------------------------------
#define FOV 1.0                    // [2.1] 视场：越小长焦(云大)，越大广角(透视强)
#define CAM_POS vec3(0.0, 1.0, 0.0)   // [2.2] 相机位置：y=眼睛高度
#define CAM_LOOK vec3(0.0, 1.6, -1.0)  // [2.3] 观察目标：y=仰角，z=远近距离感
#define CLOUD_BASE 100.0           // [2.4] 云层底部：越小云越近越大，越大越高远
#define CLOUD_THICK 90.0           // [2.5] 云层厚度（垂直纵深）
#define COVERAGE 0.3125            // [2.6] 云覆盖率：0.0~1.0，越高云越密
#define ABSORB 1.0                 // [2.7] 光吸收系数：越高云越暗越浓
#define WIND_SPEED 0.2             // [2.8] 风速乘数：控制云移动速度
#define SUN_DIR vec3(0, 0, -1)     // [2.9] 太阳方向：控制光照和阴影方向

// 派生常量（由上方参数自动推导，无需修改）
#define cld_march_steps MARCH_STEPS
#define cld_coverage COVERAGE
#define cld_thick CLOUD_THICK
#define cld_absorb_coeff ABSORB
#define cld_wind_dir vec3(0, 0, -iTime * WIND_SPEED)
#define cld_sun_dir normalize(SUN_DIR)

// ----------------------------------------------------------------------------
// [3] 数据结构
// ----------------------------------------------------------------------------
struct ray_t {
	vec3 origin;      // [3.1] 射线起点（相机位置）
	vec3 direction;   // [3.2] 射线方向（已归一化）
};

struct volume_sampler_t {
	vec3 origin;      // [3.3] 步进起点（云层入口）
	vec3 pos;         // [3.4] 当前采样位置
	float height;     // [3.5] 归一化高度 [0,1]，用于高度梯度光照
	float coeff_absorb; // [3.6] 吸收系数
	float T;          // [3.7] 透射率 (Beer-Lambert)
	vec3 C;           // [3.8] 累积颜色
	float alpha;      // [3.9] 累积不透明度
};

// ----------------------------------------------------------------------------
// [4] 数学工具
// ----------------------------------------------------------------------------
vec3 linear_to_srgb(vec3 color) {
	// [4.1] Gamma 校正：线性颜色空间 -> sRGB
	const float p = 1.0 / 2.2;
	return vec3(pow(color.r, p), pow(color.g, p), pow(color.b, p));
}

// ----------------------------------------------------------------------------
// [5] 相位函数（根据 ENABLE_PHASE_FUNCTION 开关编译不同代码）
// ----------------------------------------------------------------------------
#if ENABLE_PHASE_FUNCTION
// [5.1] Henyey-Greenstein 相位函数：各向异性散射
// g=0.76 表示强前向散射，适合模拟真实云层对阳光的散射
float phase_function(float mu) {
	const float g = 0.76;
	return (1.0 - g*g)
		/ ((4.0 * 3.14159265359) * pow(1.0 + g*g - 2.0*g*mu, 1.5));
}
#else
// [5.2] 各向同性相位：均匀散射，无方向偏好，计算量更小
float phase_function(float mu) {
	return 1.0 / (4.0 * 3.14159265359);
}
#endif

// ----------------------------------------------------------------------------
// [6] Simplex Noise 3D (by Ian McEwan, Ashima Arts)
// 纯数学实现，无纹理依赖
// ----------------------------------------------------------------------------
vec3 mod289(vec3 x) { return x - floor(x * (1.0 / 289.0)) * 289.0; }
vec4 mod289(vec4 x) { return x - floor(x * (1.0 / 289.0)) * 289.0; }
vec4 permute(vec4 x) { return mod289(((x*34.0)+1.0)*x); }

vec4 taylorInvSqrt(vec4 r) {
	return 1.79284291400159 - 0.85373472095314 * r;
}

float snoise(vec3 v) {
	// [6.1] 3D Simplex 噪声核心：将空间划分为单形网格
	const vec2 C = vec2(1.0/6.0, 1.0/3.0);
	const vec4 D = vec4(0.0, 0.5, 1.0, 2.0);

	// [6.2] 偏斜取整，确定所在单形
	vec3 i = floor(v + dot(v, C.yyy));
	vec3 x0 = v - i + dot(i, C.xxx);

	// [6.3] 确定单形其余三个角点
	vec3 g = step(x0.yzx, x0.xyz);
	vec3 l = 1.0 - g;
	vec3 i1 = min(g.xyz, l.zxy);
	vec3 i2 = max(g.xyz, l.zxy);

	vec3 x1 = x0 - i1 + C.xxx;
	vec3 x2 = x0 - i2 + C.yyy;
	vec3 x3 = x0 - D.yyy;

	// [6.4] 置换哈希：生成伪随机梯度索引
	i = mod289(i);
	vec4 p = permute(permute(permute(
		i.z + vec4(0.0, i1.z, i2.z, 1.0))
		+ i.y + vec4(0.0, i1.y, i2.y, 1.0))
		+ i.x + vec4(0.0, i1.x, i2.x, 1.0));

	// [6.5] 梯度生成与归一化
	float n_ = 0.142857142857;
	vec3 ns = n_ * D.wyz - D.xzx;

	vec4 j = p - 49.0 * floor(p * ns.z * ns.z);

	vec4 x_ = floor(j * ns.z);
	vec4 y_ = floor(j - 7.0 * x_);
	vec4 x = x_ * ns.x + ns.yyyy;
	vec4 y = y_ * ns.x + ns.yyyy;
	vec4 h = 1.0 - abs(x) - abs(y);

	vec4 b0 = vec4(x.xy, y.xy);
	vec4 b1 = vec4(x.zw, y.zw);

	vec4 s0 = floor(b0)*2.0 + 1.0;
	vec4 s1 = floor(b1)*2.0 + 1.0;
	vec4 sh = -step(h, vec4(0.0));

	vec4 a0 = b0.xzyw + s0.xzyw*sh.xxyy;
	vec4 a1 = b1.xzyw + s1.xzyw*sh.zzww;

	vec3 p0 = vec3(a0.xy, h.x);
	vec3 p1 = vec3(a0.zw, h.y);
	vec3 p2 = vec3(a1.xy, h.z);
	vec3 p3 = vec3(a1.zw, h.w);

	vec4 norm = taylorInvSqrt(vec4(dot(p0,p0), dot(p1,p1), dot(p2,p2), dot(p3,p3)));
	p0 *= norm.x; p1 *= norm.y; p2 *= norm.z; p3 *= norm.w;

	// [6.6] 混合最终噪声值（基于距离平方的权重）
	vec4 m = max(0.6 - vec4(dot(x0,x0), dot(x1,x1), dot(x2,x2), dot(x3,x3)), 0.0);
	m = m * m;
	return 42.0 * dot(m*m, vec4(dot(p0,x0), dot(p1,x1), dot(p2,x2), dot(p3,x3)));
}

// ----------------------------------------------------------------------------
// [7] FBM (分形布朗运动) —— 八度数由 FBM_OCTAVES 宏控制
// ----------------------------------------------------------------------------
#define DECL_FBM_FUNC(_name, _octaves, _basis) \
	float _name(vec3 pos, float lacunarity, float init_gain, float gain) { \
		vec3 p = pos; float H = init_gain; float t = 0.; \
		for (int i = 0; i < _octaves; i++) { \
			t += _basis * H; p *= lacunarity; H *= gain; \
		} return t; \
	}

// [7.1] 使用绝对值噪声（abs(snoise)）适合生成云团这种"blobby"形态
DECL_FBM_FUNC(fbm_clouds, FBM_OCTAVES, abs(snoise(p)))

// ----------------------------------------------------------------------------
// [8] 体积渲染核心（根据 ENABLE_BEER_LAMBERT 开关编译不同积分方式）
// ----------------------------------------------------------------------------
volume_sampler_t begin_volume(vec3 origin, float coeff_absorb) {
	// [8.1] 初始化体积采样器：透射率=1（完全透明），颜色/不透明度=0
	return volume_sampler_t(
		origin, origin, 0.0,
		coeff_absorb, 1.0,
		vec3(0.0), 0.0
	);
}

// [8.2] 体积内光照：高度梯度（底部暗顶部亮）+ 可选相位函数
float illuminate_volume(volume_sampler_t vol, vec3 V, vec3 L) {
	float mu = dot(V, L);              // 视线与光源夹角余弦
	float phase = phase_function(mu); // 散射相位
	return exp(vol.height) / 1.95 * phase;
}

// [8.3] 体积积分：根据 ENABLE_BEER_LAMBERT 开关编译不同代码路径
#if ENABLE_BEER_LAMBERT
void integrate_volume(inout volume_sampler_t vol, vec3 V, vec3 L, float density, float dt) {
	// Beer-Lambert 模式：物理透射累积，有体积阴影感
	// 注意：dt 过大时 exp() 会断崖式衰减，需配合 MAX_STEP 限制
	float T_i = exp(-vol.coeff_absorb * density * dt);
	vol.T *= T_i;
	vol.C += vol.T * illuminate_volume(vol, V, L) * density * dt;
	vol.alpha += (1.0 - T_i) * (1.0 - vol.alpha);
}
#else
void integrate_volume(inout volume_sampler_t vol, vec3 V, vec3 L, float density, float dt) {
	// 简化模式：无透射衰减，直接颜色线性叠加
	// 性能更高，但无体积阴影，地平线不易出现白线
	float light = illuminate_volume(vol, V, L);
	float contrib = density * dt * 0.05;
	vol.C += light * contrib;
	vol.alpha += contrib;
	vol.alpha = min(vol.alpha, 1.0);
}
#endif

// ----------------------------------------------------------------------------
// [9] 云密度函数
// ----------------------------------------------------------------------------
float density_func(vec3 pos, float h) {
	// [9.1] 应用世界缩放和风偏移
	vec3 p = pos * 0.001 + cld_wind_dir;
	// [9.2] FBM 生成基础密度
	float dens = fbm_clouds(p * 2.032, 2.6434, 0.5, 0.5);
	// [9.3] 阈值裁剪：低于覆盖率的部分清零，形成云洞和边缘
	dens *= smoothstep(cld_coverage, cld_coverage + 0.035, dens);
	return dens;
}

// ----------------------------------------------------------------------------
// [10] 云渲染主函数（步进数由 MARCH_STEPS 宏控制）
// ----------------------------------------------------------------------------
vec4 render_clouds(ray_t eye) {
	const int steps = cld_march_steps;
	float cloud_base = CLOUD_BASE;
	float cloud_top = cloud_base + cld_thick;

	// [10.1] 只处理向上看的射线（地面以下不渲染）
	if (eye.direction.y <= 0.001) return vec4(0.0);

	// [10.2] 计算射线与云层底部/顶部的实际交点距离
	float t_enter = (cloud_base - eye.origin.y) / eye.direction.y;
	float t_exit  = (cloud_top - eye.origin.y) / eye.direction.y;

	// [10.3] 无交点或云层在身后则返回透明
	if (t_enter > t_exit || t_exit < 0.0) return vec4(0.0);
	t_enter = max(t_enter, 0.0);

	// [10.4] 在云层区间内均匀步进，但限制最大步长
	// 当视线接近水平时，t_exit-t_enter 极大，步长会爆炸
	// MAX_STEP 防止 Beer-Lambert 模式因 dt 过大产生地平线白线
	float step_size = (t_exit - t_enter) / float(steps);
	const float MAX_STEP = 4.0;          // [10.5] 单步最大距离限制
	step_size = min(step_size, MAX_STEP);

	vec3 iter = eye.direction * step_size;
	vec3 start_pos = eye.origin + eye.direction * t_enter;

	volume_sampler_t cloud = begin_volume(start_pos, cld_absorb_coeff);

	// [10.6] 循环条件改为距离控制，而非固定步数
	// 当 MAX_STEP 生效时，实际步进距离可能超过云层厚度，需用 traveled 控制
	float traveled = 0.0;
	float total_dist = t_exit - t_enter;

	for (int i = 0; i < steps; i++) {
		// [10.7] 超出云层顶部或已走完全程则退出
		if (cloud.pos.y > cloud_top || traveled > total_dist) break;

		// [10.8] 计算当前归一化高度
		cloud.height = (cloud.pos.y - cloud_base) / cld_thick;
		// [10.9] 采样密度
		float dens = density_func(cloud.pos, cloud.height);

		// [10.10] 积分体积属性
		integrate_volume(cloud, eye.direction, cld_sun_dir, dens, step_size);

		// [10.11] 步进到下一点
		cloud.pos += iter;
		traveled += step_size;

		// [10.12] 可选：Alpha 饱和时提前退出（由 ENABLE_EARLY_EXIT 控制）
		#if ENABLE_EARLY_EXIT
		if (cloud.alpha > 0.999) break;
		#endif
	}

	return vec4(cloud.C, cloud.alpha);
}

// ----------------------------------------------------------------------------
// [11] 主入口
// ----------------------------------------------------------------------------
void mainImage(out vec4 fragColor, in vec2 fragCoord) {
	// [11.1] 从 [2] 区读取相机参数
	vec3 eye = CAM_POS;
	vec3 look_at = CAM_LOOK;

	// [11.2] 构建相机坐标系
	vec3 fwd = normalize(look_at - eye);
	vec3 up = vec3(0.0, 1.0, 0.0);
	vec3 right = cross(up, fwd);
	up = cross(fwd, right);

	// [11.3] NDC -> 相机局部坐标
	vec2 aspect = vec2(iResolution.x / iResolution.y, 1.0);
	vec2 ndc = fragCoord / iResolution.xy;
	vec3 point_cam = vec3((2.0 * ndc - 1.0) * aspect * FOV, -1.0);

	// [11.4] 生成主射线
	ray_t ray;
	ray.origin = eye;
	ray.direction = normalize(fwd + up * point_cam.y + right * point_cam.x);

	// [11.5] 只渲染云，Alpha 通道保留不透明度
	vec4 cld = render_clouds(ray);
	fragColor = vec4(linear_to_srgb(cld.rgb), cld.a);
}


// --- ISF entry point: bridge to Shadertoy mainImage ---
void main() {
    mainImage(gl_FragColor, gl_FragCoord.xy);
}
